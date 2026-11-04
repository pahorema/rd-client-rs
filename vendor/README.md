# Vendored crates

Third-party crates carried in-tree because they need a fix that is not
released yet. Each one is wired in through `[patch.crates-io]` in the root
`Cargo.toml`; drop the copy once upstream ships the fix.

| Crate | Version | Local change |
|---|---|---|
| `unrar-ng` | 0.7.7 | The volume-change callback reads the next volume name up to its terminator instead of copying a fixed 2048-character window past the end of the buffer. |
