# Building

## Requirements

* MinGW-w64's 32-bit C compiler and binutils (`i686-w64-mingw32-gcc`,
  `windres`), GNU make, Python 3.
  * Debian/Ubuntu: `sudo apt install gcc-mingw-w64-i686 binutils-mingw-w64-i686 make python3`
  * Windows: MSYS2 with `mingw-w64-i686-gcc` and `make`; run
    `make CROSS=` from a MINGW32 shell, or keep the default `CROSS` prefix
    if your toolchain uses it.
* For the test suite on Linux: 32-bit Wine (`sudo dpkg --add-architecture i386 && sudo apt install wine32:i386 wine`).
* For packages: NSIS 3 (`makensis`) and msitools (`wixl`).

No other libraries are needed: the programs use only Windows itself and the
vendored miniz.

## Build and test

```
make                  # build/zipshrink.exe, build/zipshrink-gui.exe
make test             # build/zs-tests.exe, build/zipshrink-fp.exe, then tests/run.py
make check-imports    # every import must exist on Windows 95
```

`make test` runs the programs directly on Windows and through `wine`
elsewhere. To run the suite with Wine pretending to be Windows 95 (ANSI file
API, cached read-back, the 9x delete path):

```
export WINEPREFIX=$HOME/.wine-win95
wineboot -i
wine reg add 'HKCU\Software\Wine' /v Version /d win95 /f
python3 tests/run.py --build build --win95
```

`tests/run.py -k NAME` runs the tests whose names contain NAME; `--keep`
keeps the folders of failed tests; `--slow` adds a test with a file over
4 GiB.

`zipshrink-fp.exe` and `zs-tests.exe` are built with `-DZS_FAILPOINTS` (fault
injection through the `ZIPSHRINK_FAILPOINT` environment variable). Release
builds never contain it.

## Packages

```
packaging/build-windows.sh    # writes dist/
```

Produces `ZipShrink-setup.exe` (ANSI NSIS installer for Windows 95–11) and
`ZipShrink.msi` (schema 200, per machine). The release
workflow uses exactly this script.

## Regenerating the icon

```
python3 scripts/make_icon.py    # assets/zipshrink.ico (16-colour and 32-bit BMP entries)
```

## Releasing

1. Change the version in `src/version.h`.
2. Merge to `main`, then run the **Release** workflow with the tag
   `vX.Y.Z` (or push the tag). It builds the packages, writes `SHA256SUMS`,
   and publishes a GitHub release; the updater picks it up from there.
