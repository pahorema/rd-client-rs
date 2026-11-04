use std::fs;

use rdc_archive::{Error, extract, list_entries};
use tokio_util::sync::CancellationToken;

use crate::common::{big_text, copy_fixtures, copy_fixtures_as, files_below};

const MULTI: [&str; 3] = ["multi.part1.rar", "multi.part2.rar", "multi.part3.rar"];
const LEGACY: [&str; 3] = ["legacy.rar", "legacy.r00", "legacy.r01"];

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
fn new_style_multi_volume_set_is_extracted_from_its_first_volume() {
    let dir = tempfile::tempdir().unwrap();
    let archive = copy_fixtures(dir.path(), &MULTI);
    let destination = dir.path().join("out");
    let mut seen = Vec::new();

    extract(
        &archive,
        &destination,
        |percent| seen.push(percent),
        &CancellationToken::new(),
    )
    .unwrap();

    assert_eq!(
        fs::read_to_string(destination.join("big.txt")).unwrap(),
        big_text()
    );
    assert_eq!(seen, [0, 100]);
}

#[test]
fn old_style_multi_volume_set_is_extracted() {
    let dir = tempfile::tempdir().unwrap();
    let archive = copy_fixtures(dir.path(), &LEGACY);
    let destination = dir.path().join("out");

    extract_all(&archive, &destination).unwrap();

    assert_eq!(
        fs::read_to_string(destination.join("big.txt")).unwrap(),
        big_text()
    );
}

#[test]
fn numerically_named_volumes_are_followed() {
    let dir = tempfile::tempdir().unwrap();
    let archive = copy_fixtures_as(
        dir.path(),
        &[
            ("legacy.rar", "set.001"),
            ("legacy.r00", "set.002"),
            ("legacy.r01", "set.003"),
        ],
    );
    let destination = dir.path().join("out");

    extract_all(&archive, &destination).unwrap();

    assert_eq!(
        fs::read_to_string(destination.join("big.txt")).unwrap(),
        big_text()
    );
}

#[test]
fn multi_volume_listing_reports_a_split_file_once() {
    let dir = tempfile::tempdir().unwrap();
    let archive = copy_fixtures(dir.path(), &MULTI);

    let entries = list_entries(&archive).unwrap();

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].name, "big.txt");
    assert_eq!(entries[0].size, 16_500);
}

#[test]
fn a_missing_volume_fails_the_extraction() {
    let dir = tempfile::tempdir().unwrap();
    let archive = copy_fixtures(dir.path(), &MULTI[..2]);

    let error = extract_all(&archive, &dir.path().join("out")).unwrap_err();

    assert!(matches!(error, Error::Read { .. }), "{error}");
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
