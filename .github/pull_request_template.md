## Summary

## Testing

- [ ] `cargo test --workspace --features zipshrink-core/failpoints,zipshrink-cli/failpoints`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`

## Data safety

Does this change touch compression, verification, the journal, deletion or recovery? If so, explain why it cannot delete an original that is not safely archived, and which tests cover it.
