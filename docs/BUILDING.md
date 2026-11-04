# Building

## Requirements

* Rust 1.95 or newer (`rustup`).
* For the Windows installer: the `x86_64-pc-windows-gnu` target, MinGW-w64
  and NSIS 3. On Debian/Ubuntu:
  `sudo apt install mingw-w64 nsis && rustup target add x86_64-pc-windows-gnu`.
* For the Mac app: a Mac with the Xcode Command Line Tools
  (`xcode-select --install`) and both Apple targets:
  `rustup target add aarch64-apple-darwin x86_64-apple-darwin`. It cannot be
  cross-built from Linux (the `ring` crate needs the macOS SDK).
* For building and testing on Linux (development only; ZipShrink ships for
  Windows and macOS): `libxkbcommon-dev libgtk-3-dev` for the desktop app.

## Build and test

```
cargo build --release              # target/release/zipshrink(.exe) and zipshrink-gui(.exe)
cargo test --workspace --features zipshrink-core/failpoints,zipshrink-cli/failpoints
cargo clippy --workspace --all-targets --features zipshrink-core/failpoints,zipshrink-cli/failpoints -- -D warnings
cargo fmt --all --check
```

The `failpoints` feature adds the fault-injection and crash tests (it makes
the programs obey `ZIPSHRINK_FAILPOINT`). It must never be enabled for
release builds.

The Windows tests can also run on Linux under 64-bit Wine:

```
LANG=C.UTF-8 CARGO_TARGET_X86_64_PC_WINDOWS_GNU_RUNNER=wine \
  cargo test -p zipshrink-core -p zipshrink-cli -p zipshrink-update --target x86_64-pc-windows-gnu \
  --features zipshrink-core/failpoints,zipshrink-cli/failpoints
```

## The Windows installer

```
packaging/build-windows.sh            # writes dist/ZipShrink-setup.exe
```

Cross-builds both programs with `cargo build --release --target
x86_64-pc-windows-gnu` and packs them with `packaging/windows/zipshrink.nsi`.
`ZipShrink-setup.exe` is the only file a release needs; the release workflow
uses exactly this script. Build-time settings:

* `ZIPSHRINK_UPDATE_REPO=owner/repo`: the repository the updater checks
  (default `knivesbatz/zipshrink`).
* `ZIPSHRINK_UPDATE_FROM=0.0.1`: makes a build compare updates with that
  version instead of its own; CI uses it to test a real update.

## The Mac app

```
packaging/build-macos.sh [VERSION]   # writes dist/ZipShrink.dmg and dist/ZipShrink-mac.zip
packaging/macos/check-app.sh         # checks them (what CI's macos-app job runs)
```

On a Mac, builds both programs for Apple silicon and Intel with
`MACOSX_DEPLOYMENT_TARGET=11.0`, joins each pair with `lipo` into a
universal binary, and assembles `build/macos/ZipShrink.app`:

```
ZipShrink.app/Contents/
  Info.plist           from packaging/macos/Info.plist, VERSION filled in
  MacOS/zipshrink-gui  the app (CFBundleExecutable)
  MacOS/zipshrink      the command line tool
  Resources/           AppIcon.icns (sips + iconutil from assets/icon-1024.png), licenses
```

The executables are never called `ZipShrink`: on a case-insensitive disk it
would be the same file as `zipshrink`. The bundle identifier is
`io.github.knivesbatz.zipshrink`; installed copies and the updater rely on it
and on the asset name `ZipShrink-mac.zip`, so neither may change.

The app is ad-hoc signed, inside out and without `--deep`: extended
attributes are cleared (`xattr -cr`), the command line tool is signed with
the identifier `io.github.knivesbatz.zipshrink.cli`, then the bundle. It is
not notarized, so people confirm it once with **Open Anyway** (see the
README). The updater zip is made with
`ditto -c -k --norsrc --noextattr --noqtn --keepParent` and holds exactly
`ZipShrink.app`; the disk image (UDZO, HFS+) holds the app and a link to
`/Applications`.

`VERSION` (default: the workspace version) goes into `Info.plist`, which is
what the Mac updater compares. CI's updater test builds an app that claims
to be 0.0.1 with `ZIPSHRINK_UPDATE_FROM=0.0.1 packaging/build-macos.sh 0.0.1`;
the plist must be right before signing, since editing it afterwards breaks
the signature. Other settings: `ZIPSHRINK_UPDATE_REPO` as for Windows,
`ZIPSHRINK_DIST` (output folder), `ZIPSHRINK_MAC_TARGETS` (for a quick
single-architecture test build) and `CARGO_TARGET_DIR`.

`check-app.sh` checks the architectures and minimum macOS of both slices,
the signatures, the versions (natively and under Rosetta), a round trip
with deletion through the bundled tool extracted with `ditto`, the app's
start-up and file hand-over (below), the mounted disk image and the
unpacked zip.

### Smoke mode

With `ZIPSHRINK_SMOKE=<seconds>` in its environment, the app prints
`ZIPSHRINK_SMOKE ready` on stderr once its first frame is drawn and
`ZIPSHRINK_SMOKE open <path>` for every path it is handed (command line,
another launch, Finder, Services or a drop), and closes itself after that
many seconds. `check-app.sh` starts it with `open -a ... --env
ZIPSHRINK_SMOKE=... --stderr LOG FILE` (Finder's route, the `odoc` Apple
Event), launches it a second time with a path (the single-instance inbox),
opens two more items with `open -a`, and expects every path in the log.

## Continuous integration

`.github/workflows/ci.yml` runs on every push and pull request:

| Job | Runner | What it checks |
|---|---|---|
| `lint` | Linux | `cargo fmt`, Clippy, `THIRD_PARTY_LICENSES.md` up to date |
| `lint-macos` | macOS (Apple silicon) | Clippy over the Mac-only code |
| `test` | Windows, Linux, macOS 14 (Apple silicon), macOS 15 (Intel) | the whole test suite, then a round trip with deletion checked by Python's `zipfile` |
| `installer`, `windows-install` | Linux, then Windows | builds the installer; installs, upgrades, restarts, uninstalls; a real update |
| `macos-app`, `mac-install` | macOS (Apple silicon) | builds and checks the Mac app (above); a real update of an app in `/Applications` |

The Intel Mac runner is `macos-15-intel`, GitHub's last Intel image. When it
is retired, the x86_64 tests can run on Apple silicon under Rosetta with
`cargo test --target x86_64-apple-darwin`.

## Regenerating files

```
python3 scripts/make_icon.py      # assets/icon-1024.png, icon-256.png, icon-48.png, zipshrink.ico
python3 scripts/third_party.py    # THIRD_PARTY_LICENSES.md and the license allow-list check
```

`third_party.py` lists the crates of every shipped target (Windows and both
Mac architectures); CI fails if the committed file is out of date, so run
it after changing dependencies.

## Releasing

1. Change `version` in the workspace `Cargo.toml` and run `cargo update -w`
   so `Cargo.lock` matches.
2. Merge to `main`, then run the **Release** workflow with the tag `vX.Y.Z`
   (or push the tag). Its `version` job checks the tag matches the version;
   `windows` builds the installer on Linux and `macos` builds and checks the
   Mac app on a Mac, at the same time; `publish` then adds `SHA256SUMS` (for
   the updater of 0.2 and 0.3; the installer's line first) and publishes the
   GitHub release with exactly four files: `ZipShrink-setup.exe`,
   `ZipShrink.dmg`, `ZipShrink-mac.zip` and `SHA256SUMS`. If either build
   fails, nothing is published. A draft left by a failed run, or a release
   made by hand, is completed and published instead.
3. Once the new release is published, holds exactly those four files, and
   each has been downloaded back and matched against the build and GitHub's
   recorded SHA-256, the workflow **deletes every other release** (drafts included),
   so the releases page offers one download. Git tags are kept. Installed
   copies find the new release within a week and update themselves.
