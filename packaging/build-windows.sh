#!/bin/sh
# Builds the Windows installer into dist/:
#   ZipShrink-setup.exe   for Windows 95 through 11 (also what the updater runs)
# Needs: make, i686-w64-mingw32-gcc, makensis.
set -eu
cd "$(dirname "$0")/.."
version=$(sed -n 's/^#define ZS_VERSION "\(.*\)"/\1/p' src/version.h)

make -j4 B=build all
python3 scripts/check_imports.py build/zipshrink.exe build/zipshrink-gui.exe

rm -rf dist
mkdir -p dist
(cd packaging/windows && makensis -V2 -DVERSION="$version" -DBINDIR=../../build -DOUTFILE=../../dist/ZipShrink-setup.exe zipshrink.nsi)

ls -l dist
