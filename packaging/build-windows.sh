#!/bin/sh
# Builds the Windows packages into dist/:
#   ZipShrink-setup.exe   installer for Windows 95 through 11
#   ZipShrink.msi         for msiexec and deployment tools
# Needs: make, i686-w64-mingw32-gcc, makensis, wixl (msitools).
set -eu
cd "$(dirname "$0")/.."
version=$(sed -n 's/^#define ZS_VERSION "\(.*\)"/\1/p' src/version.h)
repo=${ZIPSHRINK_UPDATE_REPO:-knivesbatz/zipshrink}

make -j4 B=build all
python3 scripts/check_imports.py build/zipshrink.exe build/zipshrink-gui.exe

rm -rf dist
mkdir -p dist
(cd packaging/windows && makensis -V2 -DVERSION="$version" -DBINDIR=../../build -DOUTFILE=../../dist/ZipShrink-setup.exe zipshrink.nsi)
(cd packaging/windows && wixl -a x86 -D Version="$version" -D BinDir=../../build -D Repo="$repo" -o ../../dist/ZipShrink.msi zipshrink.wxs)

ls -l dist
