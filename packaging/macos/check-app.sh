#!/usr/bin/env bash
# Checks the Mac build that packaging/build-macos.sh just made, on a Mac
# (CI's macos-app job, and the release before anything is published):
#   * both programs are universal (arm64 and x86_64) and need macOS 11.0;
#   * the ad-hoc signatures, identifiers and Info.plist are right;
#   * the command line tool runs natively and under Rosetta, and reports the
#     plist's version, and a round trip with deletion through it extracts
#     with ditto to the same files;
#   * the app starts (ZIPSHRINK_SMOKE), takes files from Finder's route
#     (`open -a`, the odoc Apple Event) and from a second launch (the
#     single-instance inbox), and closes again;
#   * the disk image mounts read-only with the app and an Applications link,
#     and the updater zip holds exactly ZipShrink.app, both identical to the
#     build and validly signed.
#
#   packaging/macos/check-app.sh        (from anywhere; uses build/ and dist/)
set -euo pipefail
cd "$(dirname "$0")/../.."
ROOT="$(pwd -P)"
APP="$ROOT/build/macos/ZipShrink.app"
DIST="${ZIPSHRINK_DIST:-dist}"
DIST="$(cd "$DIST" && pwd -P)"
ID=io.github.knivesbatz.zipshrink
Z="$APP/Contents/MacOS/zipshrink"
GUI="$APP/Contents/MacOS/zipshrink-gui"
# Physical path (/var is a link to /private/var), so the paths the app
# reports compare equal to the ones given to it.
T="$(mktemp -d "${RUNNER_TEMP:-/tmp}/zscheck.XXXXXX")"
T="$(cd "$T" && pwd -P)"
LOG="$T/gui.log"
MNT="$T/mnt"

fail() {
  echo "::error title=Mac app check::$*"
  exit 1
}
trap 'echo "::error title=Mac app check::line $LINENO failed: $BASH_COMMAND"' ERR
cleanup() {
  hdiutil detach "$MNT" -force >/dev/null 2>&1 || true
  pkill -f "ZipShrink.app/Contents/MacOS/zipshrink-gui" 2>/dev/null || true
}
trap cleanup EXIT
plist() { /usr/libexec/PlistBuddy -c "Print :$1" "$2/Contents/Info.plist"; }
running() { pgrep -f "ZipShrink.app/Contents/MacOS/zipshrink-gui" >/dev/null; }

echo "::group::Architectures and minimum macOS"
for bin in "$Z" "$GUI"; do
  archs="$(lipo -archs "$bin" | tr ' ' '\n' | sort | xargs)"
  [ "$archs" = "arm64 x86_64" ] || fail "$bin holds '$archs', not arm64 and x86_64"
  for a in arm64 x86_64; do
    minos="$(otool -arch "$a" -l "$bin" | awk '
      $1 == "cmd" { c = $2 }
      c == "LC_BUILD_VERSION" && $1 == "minos" { print $2; exit }
      c == "LC_VERSION_MIN_MACOSX" && $1 == "version" { print $2; exit }')"
    echo "$(basename "$bin") $a: minos $minos"
    [ "$minos" = 11.0 ] || fail "$(basename "$bin") ($a) needs macOS '$minos', expected 11.0"
  done
done
echo "::endgroup::"

echo "::group::Signatures and Info.plist"
codesign --verify --deep --strict --verbose=2 "$APP"
app_sig="$(codesign -dv --verbose=2 "$APP" 2>&1)"
cli_sig="$(codesign -dv --verbose=2 "$Z" 2>&1)"
echo "$app_sig"
echo "$cli_sig"
grep -qx "Signature=adhoc" <<<"$app_sig" || fail "the app is not ad-hoc signed"
grep -qx "Identifier=$ID" <<<"$app_sig" || fail "the app's signing identifier is wrong"
grep -qx "Signature=adhoc" <<<"$cli_sig" || fail "the command line tool is not ad-hoc signed"
grep -qx "Identifier=$ID.cli" <<<"$cli_sig" || fail "the command line tool's signing identifier is wrong"
plutil -lint "$APP/Contents/Info.plist"
[ "$(plist CFBundleIdentifier "$APP")" = "$ID" ] || fail "CFBundleIdentifier"
[ "$(plist CFBundleExecutable "$APP")" = zipshrink-gui ] || fail "CFBundleExecutable"
[ "$(plist LSMinimumSystemVersion "$APP")" = 11.0 ] || fail "LSMinimumSystemVersion"
[ "$(plist NSServices:0:NSMessage "$APP")" = compressFiles ] || fail "NSServices"
[ -s "$APP/Contents/Resources/AppIcon.icns" ] || fail "AppIcon.icns missing"
quarantine="$(xattr -lr "$APP" | grep com.apple.quarantine || true)"
[ -z "$quarantine" ] || fail "quarantine flags in the build: $quarantine"
echo "Gatekeeper's view (informational; an ad-hoc app is expected to be rejected):"
spctl --assess --type execute --verbose=4 "$APP" 2>&1 || true
echo "::endgroup::"

echo "::group::Versions"
version="$(plist CFBundleShortVersionString "$APP")"
native="$("$Z" --version)"
echo "Info.plist $version; native: $native"
[ "$native" = "zipshrink $version" ] || fail "zipshrink --version says '$native', Info.plist $version"
if [ "$(uname -m)" = arm64 ]; then
  intel="$(arch -x86_64 "$Z" --version)"
  echo "under Rosetta: $intel"
  [ "$intel" = "$native" ] || fail "the x86_64 slice says '$intel'"
else
  apple="$(arch -arm64 "$Z" --version 2>/dev/null || echo "not runnable here")"
  echo "arm64 slice: $apple"
fi
echo "::endgroup::"

echo "::group::Round trip with the bundled command line tool"
rt="$T/rt"
mkdir -p "$rt/src/a/b" "$rt/src/empty"
head -c 3000000 /dev/urandom >"$rt/src/a/random.bin"
head -c 2000000 < <(yes "some text") >"$rt/src/a/b/text.txt" # no pipe: `yes` would die of SIGPIPE under pipefail
echo "unicode ü 日本" >"$rt/src/a/ünï.txt"
(cd "$rt" && find src -type f -exec shasum -a 256 {} + | sort -k2) >"$rt/before.sha"
"$Z" create "$rt/src" -o "$rt/out.zip" --delete-originals --yes --min-free 16M
[ ! -e "$rt/src" ] || fail "the originals were not deleted: $(find "$rt/src")"
"$Z" verify "$rt/out.zip"
mkdir "$rt/x"
ditto -x -k "$rt/out.zip" "$rt/x"
(cd "$rt/x" && find src -type f -exec shasum -a 256 {} + | sort -k2) | diff - "$rt/before.sha"
[ -d "$rt/x/src/empty" ] || fail "the empty folder is missing from the archive"
echo "::endgroup::"

echo "::group::The app starts and takes files from Finder and from a second launch"
in="$T/in"
mkdir -p "$in/c dir"
for f in a b d; do echo "$f" >"$in/$f.txt"; done
: >"$LOG"
has_line() { grep -qxF "ZIPSHRINK_SMOKE $1" "$LOG" || grep -qxF "ZIPSHRINK_SMOKE $1/" "$LOG"; }
wait_line() {
  local i
  for ((i = 0; i < $2 * 2; i++)); do
    if has_line "$1"; then
      echo "seen: ZIPSHRINK_SMOKE $1"
      return 0
    fi
    sleep 0.5
  done
  echo "--- app log"
  cat "$LOG"
  fail "the app never logged 'ZIPSHRINK_SMOKE $1' within $2 s"
}
# 1. Started by Launch Services with a file, as Finder's "Open With" does:
#    the file arrives as an odoc Apple Event. The app closes itself after
#    ZIPSHRINK_SMOKE seconds.
open -a "$APP" --env ZIPSHRINK_SMOKE=150 --stderr "$LOG" "$in/a.txt"
wait_line ready 90
wait_line "open $in/a.txt" 30
# 2. A second launch with a path hands it to the first window through the
#    inbox and exits at once.
ZIPSHRINK_SMOKE=30 "$GUI" "$in/b.txt" 2>"$T/second.log" &
second=$!
for ((i = 0; i < 60; i++)); do
  kill -0 "$second" 2>/dev/null || break
  sleep 0.5
done
if kill -0 "$second" 2>/dev/null; then
  kill "$second" || true
  cat "$T/second.log"
  fail "a second launch kept running instead of handing its file over"
fi
status=0
wait "$second" || status=$?
[ "$status" = 0 ] || { cat "$T/second.log"; fail "the second launch exited with status $status"; }
wait_line "open $in/b.txt" 30
# 3. Files opened with the app while it runs (Finder, Dock, `open -a`).
open -a "$APP" "$in/c dir" "$in/d.txt"
wait_line "open $in/c dir" 30
wait_line "open $in/d.txt" 30
# 4. The Services entry (registered once the app has run). Informational:
#    the pasteboard server's cache on a build machine is not reliable.
/System/Library/CoreServices/pbs -update 2>/dev/null || true
services="$(/System/Library/CoreServices/pbs -dump 2>/dev/null || true)"
if grep -q "Compress with ZipShrink" <<<"$services"; then
  echo "Services lists 'Compress with ZipShrink'"
else
  echo "::warning title=Mac app check::'Compress with ZipShrink' is not in the Services list (pbs -dump)"
fi
# 5. Quit through the quit Apple Event (Dock > Quit); a build machine may
#    not allow sending it, and then the smoke timer closes the app.
if perl -e 'alarm shift; exec @ARGV' 20 osascript -e "tell application id \"$ID\" to quit"; then
  echo "sent quit"
else
  echo "::warning title=Mac app check::could not send the quit Apple Event; waiting for the smoke timer"
fi
for ((i = 0; i < 360; i++)); do
  running || break
  sleep 0.5
done
if running; then
  cat "$LOG"
  fail "the app did not close"
fi
echo "--- app log"
cat "$LOG"
if grep -qE "panicked|fatal runtime error" "$LOG"; then fail "the app panicked"; fi
echo "::endgroup::"

echo "::group::Disk image"
mkdir "$MNT"
hdiutil attach -readonly -nobrowse -noautoopen -mountpoint "$MNT" "$DIST/ZipShrink.dmg"
ls -la "$MNT"
if [ ! -L "$MNT/Applications" ] || [ "$(readlink "$MNT/Applications")" != /Applications ]; then
  fail "no Applications link in the disk image"
fi
codesign --verify --deep --strict --verbose=2 "$MNT/ZipShrink.app"
diff -r "$MNT/ZipShrink.app" "$APP"
for ((i = 0; i < 5; i++)); do
  hdiutil detach "$MNT" && break
  sleep 2
done
echo "::endgroup::"

echo "::group::Updater zip"
x="$T/zip"
mkdir "$x"
unzip -l "$DIST/ZipShrink-mac.zip"
stray="$(unzip -Z1 "$DIST/ZipShrink-mac.zip" | grep -v '^ZipShrink\.app/' || true)"
[ -z "$stray" ] || fail "the zip holds more than ZipShrink.app: $stray"
ditto -x -k "$DIST/ZipShrink-mac.zip" "$x"
[ "$(ls -A "$x")" = ZipShrink.app ] || fail "the zip unpacks to: $(ls -A "$x")"
codesign --verify --deep --strict --verbose=2 "$x/ZipShrink.app"
diff -r "$x/ZipShrink.app" "$APP"
attrs="$(xattr -lr "$x/ZipShrink.app")"
[ -z "$attrs" ] || fail "extended attributes in the zip: $attrs"
echo "::endgroup::"

rm -rf "$T"
echo "The Mac app passed every check."
