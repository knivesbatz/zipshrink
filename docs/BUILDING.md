# Building

## Requirements

* Rust 1.89 or newer (`rustup`).
* Linux GUI builds: X11/Wayland development headers, e.g. on Debian/Ubuntu
  `libxkbcommon-dev libgtk-3-dev` (GTK is used for the file dialogs).

## Build and test

```
cargo build --release              # target/release/zipshrink and zipshrink-gui
cargo test --workspace             # unit and integration tests
cargo test -p zipshrink-core -p zipshrink-cli \
  --features zipshrink-core/failpoints,zipshrink-cli/failpoints   # fault injection and crash tests
cargo clippy --workspace --all-targets
```

The `failpoints` feature must never be enabled for release builds.

## Windows release files from Linux

```
sudo apt install mingw-w64 nsis wixl zip
rustup target add x86_64-pc-windows-gnu
packaging/build-windows.sh            # writes dist/
```

Produces `ZipShrink-x64-setup.exe` (NSIS, per user), `ZipShrink-x64.msi`
(per machine), `ZipShrink-x64-portable.zip` and `zipshrink-x86_64-windows.exe`.
The release workflow uses exactly this script. To set the repository the
updater checks, export `ZIPSHRINK_UPDATE_REPO=owner/repo` before building.

The Windows test suite can be run on Linux under Wine (64-bit, with a UTF-8
locale):

```
LANG=C.UTF-8 CARGO_TARGET_X86_64_PC_WINDOWS_GNU_RUNNER=wine \
  cargo test -p zipshrink-core -p zipshrink-cli --target x86_64-pc-windows-gnu \
  --features zipshrink-core/failpoints,zipshrink-cli/failpoints
```

## Regenerating assets

```
pip install fonttools pillow numpy
python3 scripts/make_pixel_font.py /usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf
python3 scripts/make_icon.py
python3 scripts/third_party.py         # THIRD_PARTY_LICENSES.md + license allow-list check
```

## Releasing

1. Update `version` in the workspace `Cargo.toml` and `Cargo.lock`.
2. Tag `vX.Y.Z` and push the tag. The release workflow builds Windows, Linux
   and macOS files, writes `SHA256SUMS`, and publishes a GitHub release. The
   app's updater picks it up from there.
