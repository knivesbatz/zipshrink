#!/usr/bin/env bash
# Builds the Windows release files from Linux: cross-compiles with MinGW,
# then makes the setup .exe (NSIS), the .msi (wixl) and the portable .zip.
#   packaging/build-windows.sh [VERSION]
# Needs: rustup target x86_64-pc-windows-gnu, mingw-w64, nsis, wixl, zip.
set -euo pipefail
cd "$(dirname "$0")/.."
VERSION="${1:-$(cargo metadata --no-deps --format-version 1 | python3 -c 'import json,sys; print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"]=="zipshrink-cli"))')}"
TARGET=x86_64-pc-windows-gnu
BIN="target/$TARGET/release"
DIST=dist
cargo build --release --locked --target "$TARGET" -p zipshrink-cli -p zipshrink-gui
mkdir -p "$DIST"
BIN_ABS="$(pwd)/$BIN"

# Setup (per user, no admin).
( cd packaging/windows && makensis -V2 -DVERSION="$VERSION" -DBINDIR="$BIN_ABS" -DOUTFILE="$(pwd)/../../$DIST/ZipShrink-x64-setup.exe" zipshrink.nsi )

# MSI (per machine).
( cd packaging/windows && wixl -a x64 -D Version="$VERSION" -D BinDir="$BIN_ABS" -D Repo="${ZIPSHRINK_UPDATE_REPO:-knivesbatz/zipshrink}" -o "../../$DIST/ZipShrink-x64.msi" zipshrink.wxs )

# Portable zip: the marker file tells the updater not to run setup.
STAGE="$(mktemp -d)/ZipShrink"
mkdir -p "$STAGE"
cp "$BIN/zipshrink-gui.exe" "$BIN/zipshrink.exe" LICENSE THIRD_PARTY_LICENSES.md "$STAGE/"
cp README.md "$STAGE/README.txt"
printf 'This is the portable copy of ZipShrink %s.\r\nUpdates: download the new portable zip from the releases page.\r\n' "$VERSION" > "$STAGE/zipshrink-portable.txt"
rm -f "$DIST/ZipShrink-x64-portable.zip"
( cd "$(dirname "$STAGE")" && zip -q -r -X "$OLDPWD/$DIST/ZipShrink-x64-portable.zip" ZipShrink )

# Standalone command line tool.
cp "$BIN/zipshrink.exe" "$DIST/zipshrink-x86_64-windows.exe"
ls -la "$DIST"
