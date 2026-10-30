# Contributing

Thanks for helping. ZipShrink deletes people's files, so **correctness and
recoverability come before features and speed**.

* Read [docs/DATA_SAFETY.md](docs/DATA_SAFETY.md) and
  [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) first.
* Any change to compression, verification, the journal, deletion or recovery
  needs tests, including fault-injection or crash tests where a new failure
  point is introduced (see `crates/zipshrink-core/src/failpoint.rs`).
* Tests must check contents by SHA-256, never just that an archive opens.
* Run before opening a pull request:

  ```
  cargo fmt --all
  cargo clippy --workspace --all-targets --features zipshrink-core/failpoints,zipshrink-cli/failpoints -- -D warnings
  cargo test --workspace --features zipshrink-core/failpoints,zipshrink-cli/failpoints
  ```

* New dependencies must have a permissive license (`scripts/third_party.py`
  enforces an allow list) and be actively maintained.

By contributing you agree that your work is released under the MIT License.
