#!/usr/bin/env bash
# Builds the Windows installer from Linux into dist/:
#   ZipShrink-setup.exe   for 64-bit Windows 10 and 11 (also what the updater runs)
# Needs: rustup target x86_64-pc-windows-gnu and mingw-w64.
#   packaging/build-windows.sh [VERSION]
# VERSION, when given (the release workflow passes the tag's), must be the
# crates' version: the installer reports the version it was built as.
set -euo pipefail
cd "$(dirname "$0")/.."
CRATE_VERSION="$(cargo metadata --no-deps --format-version 1 | python3 -c 'import json,sys; print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"]=="zipshrink-setup"))')"
VERSION="${1:-$CRATE_VERSION}"
VERSION="${VERSION#v}"
if [ "$VERSION" != "$CRATE_VERSION" ]; then
  echo "version $VERSION does not match the crates' version $CRATE_VERSION (Cargo.toml)" >&2
  exit 1
fi
TARGET=x86_64-pc-windows-gnu
TARGET_DIR="${CARGO_TARGET_DIR:-$(pwd)/target}"
BIN="$TARGET_DIR/$TARGET/release"

# 1. The programs.
cargo build --release --locked --target "$TARGET" -p zipshrink-cli -p zipshrink-gui

# 2. Exactly the files to install; setup embeds this folder.
mkdir -p "$TARGET_DIR"
PAYLOAD="$(mktemp -d "$TARGET_DIR/windows-payload.XXXXXX")"
trap 'rm -rf "$PAYLOAD"' EXIT
cp "$BIN/zipshrink-gui.exe" "$BIN/zipshrink.exe" "$PAYLOAD/"
cp LICENSE "$PAYLOAD/LICENSE.txt"
cp THIRD_PARTY_LICENSES.md "$PAYLOAD/THIRD_PARTY_LICENSES.txt"
cp README.md "$PAYLOAD/README.txt"

# 3. The installer, which is also the uninstaller.
ZIPSHRINK_SETUP_PAYLOAD="$PAYLOAD" cargo build --release --locked --target "$TARGET" -p zipshrink-setup

rm -rf dist
mkdir -p dist
cp "$BIN/zipshrink-setup.exe" dist/ZipShrink-setup.exe

echo "dist/ZipShrink-setup.exe  ZipShrink $VERSION"
echo "  size:   $(stat -c %s dist/ZipShrink-setup.exe) bytes"
echo "  sha256: $(sha256sum dist/ZipShrink-setup.exe | cut -d' ' -f1)"
