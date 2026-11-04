//! Resolution of archive entry names and folder names to paths inside a destination directory.

use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use crate::Error;

/// Joins the folder `name` (a torrent name or hash) to `base`.
///
/// `name` is used as is, nested components included, but must stay below `base`: absolute
/// names and names with `..` components are refused. An empty name yields `base`.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "used by the unpack procedure, added next")
)]
pub(crate) fn join_name(base: &Path, name: &str) -> Result<PathBuf, Error> {
    let stays_inside = Path::new(name)
        .components()
        .all(|component| matches!(component, Component::Normal(_) | Component::CurDir));
    if !stays_inside || name.contains('\0') {
        return Err(Error::UnsafeName {
            name: name.to_owned(),
        });
    }
    Ok(base.join(name))
}

/// Resolves the archive entry `name` to a path inside `destination`.
///
/// Both `/` and `\` separate components. Empty and `.` components are ignored and `..` steps
/// back one level, but never above `destination`. Absolute names, drive prefixes and NUL bytes
/// are refused, as are names that resolve to `destination` itself.
pub(crate) fn entry_destination(destination: &Path, name: &str) -> Result<PathBuf, Error> {
    let unsafe_entry = || Error::UnsafeEntry {
        entry: name.to_owned(),
    };
    if name.starts_with(['/', '\\']) || name.contains('\0') {
        return Err(unsafe_entry());
    }
    let mut parts: Vec<&str> = Vec::new();
    for part in name.split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." => {
                if parts.pop().is_none() {
                    return Err(unsafe_entry());
                }
            }
            _ if has_drive_prefix(part) => return Err(unsafe_entry()),
            _ => parts.push(part),
        }
    }
    if parts.is_empty() {
        return Err(unsafe_entry());
    }
    let mut path = destination.to_path_buf();
    path.extend(parts);
    Ok(path)
}

/// Prepares `target` (a path returned by [`entry_destination`]) for writing a file entry.
///
/// Refuses the entry when a directory between `destination` and `target` is a symbolic link,
/// because writing through it could leave the destination. Creates the missing parent
/// directories and removes an existing file or link at `target`, so the entry replaces it.
pub(crate) fn prepare_file_target(
    destination: &Path,
    target: &Path,
    entry: &str,
) -> Result<(), Error> {
    ensure_no_linked_ancestors(destination, target, entry)?;
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(Error::io("create directory", parent))?;
    }
    match fs::symlink_metadata(target) {
        Ok(metadata) if !metadata.is_dir() => {
            fs::remove_file(target).map_err(Error::io("replace file", target))
        }
        _ => Ok(()),
    }
}

/// Creates the directory `target` (a path returned by [`entry_destination`]).
pub(crate) fn create_entry_directory(
    destination: &Path,
    target: &Path,
    entry: &str,
) -> Result<(), Error> {
    ensure_no_linked_ancestors(destination, target, entry)?;
    fs::create_dir_all(target).map_err(Error::io("create directory", target))
}

fn ensure_no_linked_ancestors(destination: &Path, target: &Path, entry: &str) -> Result<(), Error> {
    let Ok(relative) = target.strip_prefix(destination) else {
        return Err(Error::UnsafeEntry {
            entry: entry.to_owned(),
        });
    };
    let mut current = destination.to_path_buf();
    let mut components = relative.components().peekable();
    while let Some(component) = components.next() {
        if components.peek().is_none() {
            break;
        }
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(Error::UnsafeEntry {
                    entry: entry.to_owned(),
                });
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => break,
            Err(error) => return Err(Error::io("inspect", &current)(error)),
        }
    }
    Ok(())
}

/// Whether `part` starts like a Windows drive (`C:`), which would make a joined path absolute.
fn has_drive_prefix(part: &str) -> bool {
    let bytes = part.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolve(name: &str) -> Result<PathBuf, Error> {
        entry_destination(Path::new("/dest"), name)
    }

    #[test]
    fn nested_names_resolve_inside_the_destination() {
        assert_eq!(resolve("a/b/c.txt").unwrap(), Path::new("/dest/a/b/c.txt"));
        assert_eq!(
            resolve("a\\b\\c.txt").unwrap(),
            Path::new("/dest/a/b/c.txt")
        );
        assert_eq!(resolve("./a//b/").unwrap(), Path::new("/dest/a/b"));
    }

    #[test]
    fn parent_components_that_stay_inside_are_accepted() {
        assert_eq!(resolve("a/../b.txt").unwrap(), Path::new("/dest/b.txt"));
    }

    #[test]
    fn names_escaping_the_destination_are_refused() {
        for name in [
            "../evil.txt",
            "a/../../evil.txt",
            "..\\evil.txt",
            "/etc/passwd",
            "\\evil.txt",
            "C:\\evil.txt",
            "a/C:evil.txt",
            "a\0b",
            "",
            "./",
            "a/..",
        ] {
            let error = resolve(name).unwrap_err();
            assert!(
                matches!(&error, Error::UnsafeEntry { entry } if entry == name),
                "{name:?}: {error}"
            );
        }
    }

    #[test]
    fn unsafe_entry_message_names_the_entry() {
        let message = resolve("../evil.txt").unwrap_err().to_string();
        assert_eq!(
            message,
            "archive entry \"../evil.txt\" escapes the destination directory"
        );
    }

    #[cfg(unix)]
    #[test]
    fn writing_through_a_linked_directory_is_refused() {
        let outside = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), destination.path().join("link")).unwrap();
        let target = entry_destination(destination.path(), "link/file.txt").unwrap();

        let error = prepare_file_target(destination.path(), &target, "link/file.txt").unwrap_err();

        assert!(matches!(error, Error::UnsafeEntry { .. }), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn an_existing_link_at_the_target_is_replaced_not_followed() {
        let outside = tempfile::tempdir().unwrap();
        let victim = outside.path().join("victim.txt");
        fs::write(&victim, "keep").unwrap();
        let destination = tempfile::tempdir().unwrap();
        let target = destination.path().join("file.txt");
        std::os::unix::fs::symlink(&victim, &target).unwrap();

        prepare_file_target(destination.path(), &target, "file.txt").unwrap();

        assert!(!target.exists());
        assert_eq!(fs::read_to_string(&victim).unwrap(), "keep");
    }

    #[test]
    fn directory_entries_are_created_with_their_parents() {
        let destination = tempfile::tempdir().unwrap();
        let target = entry_destination(destination.path(), "a/b/").unwrap();

        create_entry_directory(destination.path(), &target, "a/b/").unwrap();

        assert!(destination.path().join("a/b").is_dir());
    }

    #[test]
    fn folder_names_are_joined_as_given() {
        let base = Path::new("/dest");
        assert_eq!(join_name(base, "Show").unwrap(), Path::new("/dest/Show"));
        assert_eq!(join_name(base, "a/b").unwrap(), Path::new("/dest/a/b"));
        assert_eq!(join_name(base, "").unwrap(), Path::new("/dest/"));
    }

    #[test]
    fn folder_names_leaving_the_base_are_refused() {
        for name in ["..", "../x", "a/../../x", "/abs", "a\0b"] {
            let error = join_name(Path::new("/dest"), name).unwrap_err();
            assert!(
                matches!(error, Error::UnsafeName { .. }),
                "{name:?}: {error}"
            );
        }
    }

    #[test]
    fn missing_parent_directories_are_created() {
        let destination = tempfile::tempdir().unwrap();
        let target = entry_destination(destination.path(), "a/b/file.txt").unwrap();

        prepare_file_target(destination.path(), &target, "a/b/file.txt").unwrap();

        assert!(destination.path().join("a/b").is_dir());
    }
}
