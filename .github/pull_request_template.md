## Summary

## Testing

- [ ] `make` and `make check-imports`
- [ ] `make test` (natively on Windows, or under Wine)
- [ ] `python3 tests/run.py --win95` with a Wine prefix in Windows 95 mode, if the change touches file access or the user interface

## Data safety

Does this change touch compression, verification, the journal, deletion or recovery? If so, explain why it cannot delete an original that is not safely archived, and which tests cover it.
