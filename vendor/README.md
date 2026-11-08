# Vendored crates

Third-party crates carried in-tree because they need a fix that is not
released yet. Each one is wired in through `[patch.crates-io]` in the root
`Cargo.toml`; drop the copy once upstream ships the fix.

| Crate | Version | Local change |
|---|---|---|
| `unrar-ng` | 0.7.7 | The volume-change callback reads the next volume name up to its terminator instead of copying a fixed 2048-character window past the end of the buffer. `FileHeader::redirection` exposes the entry's link type (`RedirType`), so links and file copies can be refused before extraction. `OpenArchive::extract_to_with_progress` reports each written block and stops the entry when its callback returns `false` (`Code::UserBreak`). |
| `unrar-ng-sys` | 0.7.7 | `build.rs` passes `/EHsc` to MSVC. Without it UnRAR's exceptions skip destructors, so a cancelled or failed extraction kept its unfinished output file open and undeleted on Windows until the process exited. |
