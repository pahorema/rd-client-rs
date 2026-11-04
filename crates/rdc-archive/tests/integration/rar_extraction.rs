use std::fs;

use rdc_archive::{Error, extract, list_entries};
use tokio_util::sync::CancellationToken;

use crate::common::{copy_fixtures, files_below};

fn extract_all(archive: &std::path::Path, destination: &std::path::Path) -> Result<(), Error> {
    extract(archive, destination, |_| {}, &CancellationToken::new())
}

#[test]
fn rar5_archive_with_nested_folders_is_extracted() {
    let dir = tempfile::tempdir().unwrap();
    let archive = copy_fixtures(dir.path(), &["nested.rar"]);
    let destination = dir.path().join("out");

    extract_all(&archive, &destination).unwrap();

    assert_eq!(
        files_below(&destination),
        ["folder/a.txt", "folder/sub/b.txt", "top.txt"]
    );
    assert_eq!(
        fs::read_to_string(destination.join("folder/sub/b.txt")).unwrap(),
        "beta\n"
    );
}

#[test]
fn rar4_archive_is_extracted() {
    let dir = tempfile::tempdir().unwrap();
    let archive = copy_fixtures(dir.path(), &["nested4.rar"]);
    let destination = dir.path().join("out");

    extract_all(&archive, &destination).unwrap();

    assert_eq!(
        files_below(&destination),
        ["folder/a.txt", "folder/sub/b.txt", "top.txt"]
    );
    assert_eq!(
        fs::read_to_string(destination.join("top.txt")).unwrap(),
        "top\n"
    );
}

#[test]
fn rar_listing_returns_files_with_sizes() {
    let dir = tempfile::tempdir().unwrap();
    let archive = copy_fixtures(dir.path(), &["nested.rar"]);

    let mut listed: Vec<(String, u64)> = list_entries(&archive)
        .unwrap()
        .into_iter()
        .map(|entry| (entry.name, entry.size))
        .collect();
    listed.sort();

    assert_eq!(
        listed,
        [
            ("folder/a.txt".to_owned(), 6),
            ("folder/sub/b.txt".to_owned(), 5),
            ("top.txt".to_owned(), 4)
        ]
    );
}

#[test]
fn existing_files_are_replaced_by_rar_entries() {
    let dir = tempfile::tempdir().unwrap();
    let archive = copy_fixtures(dir.path(), &["nested.rar"]);
    let destination = dir.path().join("out");
    fs::create_dir_all(destination.join("folder")).unwrap();
    fs::write(
        destination.join("folder/a.txt"),
        "an older and much longer file",
    )
    .unwrap();

    extract_all(&archive, &destination).unwrap();

    assert_eq!(
        fs::read_to_string(destination.join("folder/a.txt")).unwrap(),
        "alpha\n"
    );
}

#[test]
fn cancellation_is_honoured_between_rar_entries() {
    let dir = tempfile::tempdir().unwrap();
    let archive = copy_fixtures(dir.path(), &["nested.rar"]);
    let destination = dir.path().join("out");
    let cancel = CancellationToken::new();

    let result = extract(
        &archive,
        &destination,
        |percent| {
            if percent > 0 {
                cancel.cancel();
            }
        },
        &cancel,
    );

    assert!(matches!(result, Err(Error::Cancelled)), "{result:?}");
    assert_eq!(files_below(&destination).len(), 1);
}
