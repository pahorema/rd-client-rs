//! The symlink engine: links a file of a mounted provider drive into the download folder.

use std::path::{Path, PathBuf};

/// The mount directory of the configured mount path, without trailing separators.
fn parse_mount_path(configured: &str) -> PathBuf {
    PathBuf::from(configured.trim_end_matches(['\\', '/']))
}

/// Directories (relative to a search root, or absolute) that may contain the file, in order:
/// the expected directory, the names of its ancestors below the mount (deepest first), a
/// directory named like the file, one named like the file without extension, and the root.
fn candidate_directories(
    mount: &Path,
    search_path: &Path,
    file_name: &str,
    stem: &str,
) -> Vec<PathBuf> {
    let mut candidates = vec![search_path.to_owned()];
    let mut directory = search_path.to_owned();
    while let Some(parent) = directory.parent() {
        let Some(name) = directory.file_name() else {
            break;
        };
        candidates.push(PathBuf::from(name));
        directory = parent.to_owned();
        if directory == mount {
            break;
        }
    }
    candidates.extend([
        PathBuf::from(file_name),
        PathBuf::from(stem),
        PathBuf::new(),
    ]);
    let mut distinct: Vec<PathBuf> = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        if !distinct.contains(&candidate) {
            distinct.push(candidate);
        }
    }
    distinct
}

async fn find_file(root: &Path, candidates: &[PathBuf], file_name: &str) -> Option<PathBuf> {
    for candidate in candidates {
        let path = root.join(candidate).join(file_name);
        tracing::trace!("Searching {}", path.display());
        if is_file(&path).await {
            return Some(path);
        }
    }
    None
}

async fn is_file(path: &Path) -> bool {
    tokio::fs::metadata(path)
        .await
        .is_ok_and(|metadata| metadata.is_file())
}

async fn is_dir(path: &Path) -> bool {
    tokio::fs::metadata(path)
        .await
        .is_ok_and(|metadata| metadata.is_dir())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mount_path_drops_trailing_separators() {
        assert_eq!(parse_mount_path("/mnt/rd/"), PathBuf::from("/mnt/rd"));
        assert_eq!(parse_mount_path("/mnt/rd"), PathBuf::from("/mnt/rd"));
    }

    #[test]
    fn candidates_cover_expected_dir_ancestors_names_and_root() {
        let mount = Path::new("/mnt/rd");
        let candidates = candidate_directories(
            mount,
            &mount.join("Show/Season 1"),
            "episode.mkv",
            "episode",
        );
        assert_eq!(
            candidates,
            vec![
                PathBuf::from("/mnt/rd/Show/Season 1"),
                PathBuf::from("Season 1"),
                PathBuf::from("Show"),
                PathBuf::from("episode.mkv"),
                PathBuf::from("episode"),
                PathBuf::new(),
            ]
        );
    }

    #[test]
    fn candidates_are_distinct() {
        let mount = Path::new("/mnt/rd");
        let candidates = candidate_directories(mount, &mount.join("file"), "file", "file");
        assert_eq!(
            candidates,
            vec![
                PathBuf::from("/mnt/rd/file"),
                PathBuf::from("file"),
                PathBuf::new()
            ]
        );
    }

    fn touch(path: &Path) {
        std::fs::create_dir_all(path.parent().expect("parent")).expect("dirs");
        std::fs::write(path, b"x").expect("file");
    }

    #[tokio::test]
    async fn file_is_found_in_the_first_matching_candidate() {
        let root = tempfile::tempdir().expect("temp dir");
        let mount = root.path();
        let candidates = candidate_directories(mount, &mount.join("Show/Season 1"), "e.mkv", "e");
        assert_eq!(find_file(mount, &candidates, "e.mkv").await, None);
        touch(&mount.join("e.mkv"));
        assert_eq!(
            find_file(mount, &candidates, "e.mkv").await,
            Some(mount.join("e.mkv"))
        );
        touch(&mount.join("e/e.mkv"));
        assert_eq!(
            find_file(mount, &candidates, "e.mkv").await,
            Some(mount.join("e/e.mkv"))
        );
        touch(&mount.join("Show/e.mkv"));
        assert_eq!(
            find_file(mount, &candidates, "e.mkv").await,
            Some(mount.join("Show/e.mkv"))
        );
        touch(&mount.join("Show/Season 1/e.mkv"));
        assert_eq!(
            find_file(mount, &candidates, "e.mkv").await,
            Some(mount.join("Show/Season 1/e.mkv"))
        );
    }

    #[tokio::test]
    async fn directories_are_not_files() {
        let root = tempfile::tempdir().expect("temp dir");
        std::fs::create_dir(root.path().join("e.mkv")).expect("dir");
        assert!(!is_file(&root.path().join("e.mkv")).await);
        assert!(is_dir(&root.path().join("e.mkv")).await);
    }
}
