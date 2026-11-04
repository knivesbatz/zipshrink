#!/usr/bin/env bash
# Builds the Windows installer from Linux into dist/:
#   ZipShrink-setup.exe   for 64-bit Windows 10 and 11 (also what the updater runs)
# Needs: rustup target x86_64-pc-windows-gnu, mingw-w64, nsis.
#   packaging/build-windows.sh [VERSION]
set -euo pipefail
cd "$(dirname "$0")/.."
VERSION="${1:-$(cargo metadata --no-deps --format-version 1 | python3 -c 'import json,sys; print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"]=="zipshrink-cli"))')}"
TARGET=x86_64-pc-windows-gnu
BIN="$(pwd)/target/$TARGET/release"

cargo build --release --locked --target "$TARGET" -p zipshrink-cli -p zipshrink-gui

rm -rf dist
mkdir -p dist
(cd packaging/windows && makensis -V2 -DVERSION="$VERSION" -DBINDIR="$BIN" -DOUTFILE="../../dist/ZipShrink-setup.exe" zipshrink.nsi)

ls -l dist
