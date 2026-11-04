# Building

## Requirements

* Rust 1.89 or newer (`rustup`).
* For the installer: the `x86_64-pc-windows-gnu` target, MinGW-w64 and
  NSIS 3. On Debian/Ubuntu:
  `sudo apt install mingw-w64 nsis && rustup target add x86_64-pc-windows-gnu`.
* For building and testing on Linux (development only; ZipShrink ships for
  Windows): `libxkbcommon-dev libgtk-3-dev` for the desktop app.

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

## The installer

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

## Regenerating files

```
python3 scripts/make_icon.py      # assets/zipshrink.ico
python3 scripts/third_party.py    # THIRD_PARTY_LICENSES.md and the license allow-list check
```

## Releasing

1. Change `version` in the workspace `Cargo.toml` and run `cargo update -w`
   so `Cargo.lock` matches.
2. Merge to `main`, then run the **Release** workflow with the tag `vX.Y.Z`
   (or push the tag). It checks the tag matches the version, builds the
   installer, adds `SHA256SUMS` (for the updater of 0.2 and 0.3) and publishes
   the GitHub release. A draft left by a failed run, or a release made by
   hand, is completed and published instead.
3. Once the new release is published and its files are downloaded back and
   checked, the workflow **deletes every other release** (drafts included),
   so the releases page offers one download. Git tags are kept. Installed
   copies find the new release within a week and update themselves.
