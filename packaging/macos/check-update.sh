#!/usr/bin/env bash
# The Mac updater test (CI's mac-install job): an app that claims to be
# 0.0.1, built with
#   ZIPSHRINK_UPDATE_FROM=0.0.1 packaging/build-macos.sh 0.0.1
# is installed in /Applications, and its `zipshrink update` must replace it
# with the latest release: verified, validly signed, without quarantine
# flags and without leftovers. A latest release without a Mac app (made
# before macOS support) must be reported as no update, changing nothing.
#
#   packaging/macos/check-update.sh OLD_APP_ZIP OWNER/REPO
# Needs GH_TOKEN for the release lookup.
set -euo pipefail
old_zip="$1"
repo="$2"
DEST=/Applications/ZipShrink.app
Z="$DEST/Contents/MacOS/zipshrink"

fail() {
  echo "::error title=Mac update::$*"
  exit 1
}
trap 'echo "::error title=Mac update::line $LINENO failed: $BASH_COMMAND"' ERR
version() { /usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$DEST/Contents/Info.plist"; }

rm -rf "$DEST"
ditto -x -k "$old_zip" /Applications
[ "$(version)" = 0.0.1 ] || fail "the old app is $(version), not 0.0.1"
codesign --verify --deep --strict "$DEST"

rel="$(gh api "repos/$repo/releases/latest")"
tag="$(jq -r .tag_name <<<"$rel")"
want="${tag#v}"
echo "latest release: $tag"

status=0
out="$("$Z" update 2>&1)" || status=$?
echo "$out"
if grep -q "limiting requests" <<<"$out"; then
  echo "::warning title=Mac update::GitHub rate limit; updater test skipped"
  exit 0
fi

if ! jq -e '.assets[] | select(.name == "ZipShrink-mac.zip")' <<<"$rel" >/dev/null; then
  [ "$status" = 0 ] || fail "zipshrink update failed ($status) for a release without a Mac app"
  [ "$(version)" = 0.0.1 ] || fail "the app changed to $(version) although $tag has no Mac app"
  echo "::warning title=Mac update::release $tag has no ZipShrink-mac.zip yet; only the no-update path was tested"
  exit 0
fi

[ "$status" = 0 ] || fail "zipshrink update failed with status $status"
grep -q "Verified the SHA-256" <<<"$out" || fail "the download was not verified"
# The new app is in place when the command returns; a background finisher
# then removes the old copy once the command has exited.
for ((i = 0; i < 60; i++)); do
  [ "$(version 2>/dev/null || true)" = "$want" ] && break
  sleep 1
done
[ "$(version)" = "$want" ] || fail "Info.plist says $(version), expected $want"
left=""
for ((i = 0; i < 60; i++)); do
  left="$(ls -d /Applications/.ZipShrink-update-* "$HOME/Library/Caches/ZipShrink/update"/* 2>/dev/null || true)"
  [ -z "$left" ] && break
  sleep 1
done
[ -z "$left" ] || fail "the update left behind: $left"
codesign --verify --deep --strict --verbose=2 "$DEST"
quarantine="$(xattr -lr "$DEST" | grep com.apple.quarantine || true)"
[ -z "$quarantine" ] || fail "the updated app is quarantined: $quarantine"
new="$("$Z" --version)"
[ "$new" = "zipshrink $want" ] || fail "the updated command line tool says '$new', expected $want"
echo "Updated 0.0.1 to $want."
