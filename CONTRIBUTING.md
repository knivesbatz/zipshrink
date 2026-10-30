# Contributing

Thanks for helping. ZipShrink deletes people's files, so **correctness and
recoverability come before features and speed**.

* Read [docs/DATA_SAFETY.md](docs/DATA_SAFETY.md) and
  [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) first.
* Any change to compression, verification, the journal, deletion or recovery
  needs tests, including crash tests where a new failure point is introduced
  (`zs_fail("name")` in the code, listed in `FAILPOINTS` in `tests/run.py`).
* Tests must check contents by SHA-256, never just that an archive opens.
* Keep Windows 95 working: no C runtime functions, and any Windows function
  newer than Windows 95 (or any "W" function) must be looked up with
  `zs_proc()` and used only when present. `make check-imports` enforces this.
* Run before opening a pull request (on Linux this needs 32-bit Wine):

  ```
  make
  make check-imports
  make test
  ```

  and, if you can, the suite in Windows 95 mode (see docs/BUILDING.md).
* New third-party code must have a permissive license and be listed in
  THIRD_PARTY_LICENSES.md.

By contributing you agree that your work is released under the MIT License.
