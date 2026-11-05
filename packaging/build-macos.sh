#!/usr/bin/env bash
# Builds the Mac app on a Mac into dist/:
#   ZipShrink.dmg       for people: open it, drag ZipShrink to Applications
#   ZipShrink-mac.zip   the same app for the updater (installed copies look
#                       for exactly this name: never rename it)
# Both hold one universal ZipShrink.app (Apple silicon and Intel, macOS 11 or
# later), ad-hoc signed: zipshrink-gui is the app, zipshrink the command line.
# Needs: Xcode Command Line Tools, and
#   rustup target add aarch64-apple-darwin x86_64-apple-darwin
#
#   packaging/build-macos.sh [VERSION]
#
# VERSION goes into Info.plist (default: the workspace version). The updater
# compares the plist's version, so CI builds a 0.0.1 app to test an update.
# Optional settings: ZIPSHRINK_UPDATE_REPO / ZIPSHRINK_UPDATE_FROM (as for
# Windows), CARGO_TARGET_DIR, ZIPSHRINK_DIST (output folder, default dist),
# ZIPSHRINK_MAC_TARGETS (default both targets; a test build may use one).
# The unpacked app is left in build/macos/ZipShrink.app.
set -euo pipefail
cd "$(dirname "$0")/.."
VERSION="${1:-$(cargo metadata --no-deps --format-version 1 | python3 -c 'import json,sys; print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"]=="zipshrink-cli"))')}"
if ! [[ $VERSION =~ ^[0-9]+(\.[0-9]+){0,2}([-+][0-9A-Za-z.-]+)?$ ]]; then
  echo "not a version: $VERSION" >&2
  exit 2
fi
BUILD="${VERSION%%[-+]*}"  # CFBundleVersion: digits and dots only
read -r -a TARGETS <<<"${ZIPSHRINK_MAC_TARGETS:-aarch64-apple-darwin x86_64-apple-darwin}"
TARGET_DIR="${CARGO_TARGET_DIR:-target}"
DIST="${ZIPSHRINK_DIST:-dist}"
WORK=build/macos
APP="$WORK/ZipShrink.app"
CLI_ID=io.github.knivesbatz.zipshrink.cli

# The oldest macOS the programs run on; matches LSMinimumSystemVersion.
export MACOSX_DEPLOYMENT_TARGET=11.0

target_args=()
for t in "${TARGETS[@]}"; do target_args+=(--target "$t"); done
cargo build --release --locked "${target_args[@]}" -p zipshrink-cli -p zipshrink-gui

rm -rf "$WORK"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources" "$DIST"
DIST="$(cd "$DIST" && pwd)"
rm -f "$DIST/ZipShrink.dmg" "$DIST/ZipShrink-mac.zip"

# One universal binary per program.
archs=()
for t in "${TARGETS[@]}"; do
  case "$t" in
    aarch64-apple-darwin) archs+=(arm64) ;;
    x86_64-apple-darwin) archs+=(x86_64) ;;
    *) echo "unexpected target $t" >&2; exit 2 ;;
  esac
done
for bin in zipshrink zipshrink-gui; do
  slices=()
  for t in "${TARGETS[@]}"; do slices+=("$TARGET_DIR/$t/release/$bin"); done
  lipo -create -output "$APP/Contents/MacOS/$bin" "${slices[@]}"
  lipo "$APP/Contents/MacOS/$bin" -verify_arch "${archs[@]}"
done

sed -e "s/@VERSION@/$VERSION/g" -e "s/@BUILD@/$BUILD/g" packaging/macos/Info.plist >"$APP/Contents/Info.plist"
plutil -lint "$APP/Contents/Info.plist"
printf 'APPL????' >"$APP/Contents/PkgInfo"
cp LICENSE "$APP/Contents/Resources/LICENSE.txt"
cp THIRD_PARTY_LICENSES.md "$APP/Contents/Resources/THIRD_PARTY_LICENSES.md"

# AppIcon.icns from the 1024 px rendering (scripts/make_icon.py).
ICONSET="$WORK/AppIcon.iconset"
mkdir -p "$ICONSET"
for s in 16 32 128 256 512; do
  sips -z "$s" "$s" assets/icon-1024.png --out "$ICONSET/icon_${s}x${s}.png" >/dev/null
  sips -z $((s * 2)) $((s * 2)) assets/icon-1024.png --out "$ICONSET/icon_${s}x${s}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/AppIcon.icns"
rm -rf "$ICONSET"

# Ad-hoc signature, inside out (no --deep): the command line tool first,
# then the bundle, which signs the app's executable and seals the rest.
# Extended attributes (Finder info, quarantine) would break the seal.
xattr -cr "$APP"
codesign --force --sign - --identifier "$CLI_ID" "$APP/Contents/MacOS/zipshrink"
codesign --force --sign - "$APP"
codesign --verify --deep --strict --verbose=2 "$APP"

# For the updater: a zip holding exactly ZipShrink.app, with no extended
# attributes, resource forks or quarantine flags.
ditto -c -k --norsrc --noextattr --noqtn --keepParent "$APP" "$DIST/ZipShrink-mac.zip"

# For people: a compressed disk image with the app and a link to
# /Applications to drag it onto. hdiutil sometimes fails with "Resource
# busy" on build machines, so it gets a few tries.
STAGE="$WORK/dmg"
mkdir -p "$STAGE"
ditto --norsrc --noextattr --noqtn "$APP" "$STAGE/ZipShrink.app"
ln -s /Applications "$STAGE/Applications"
for try in 1 2 3 4 5; do
  if hdiutil create -volname ZipShrink -srcfolder "$STAGE" -fs HFS+ -format UDZO -imagekey zlib-level=9 -ov "$DIST/ZipShrink.dmg"; then
    break
  fi
  if [ "$try" = 5 ]; then
    echo "hdiutil create failed $try times" >&2
    exit 1
  fi
  sleep $((try * 5))
done
hdiutil verify "$DIST/ZipShrink.dmg"
rm -rf "$STAGE"

(cd "$DIST" && ls -l ZipShrink.dmg ZipShrink-mac.zip && shasum -a 256 ZipShrink.dmg ZipShrink-mac.zip)
