use std::fs;

use rdc_archive::{Error, list_entries};

use crate::common::write_zip;

#[test]
fn listing_returns_files_without_directories() {
    let dir = tempfile::tempdir().unwrap();
    let archive = dir.path().join("archive.zip");
    write_zip(
        &archive,
        &[
            ("folder/", b""),
            ("folder/a.txt", b"alpha"),
            ("b.txt", b"be"),
        ],
    );

    let entries = list_entries(&archive).unwrap();

    let listed: Vec<(&str, u64)> = entries
        .iter()
        .map(|entry| (entry.name.as_str(), entry.size))
        .collect();
    assert_eq!(listed, [("folder/a.txt", 5), ("b.txt", 2)]);
}

#[test]
fn files_that_are_not_archives_are_reported() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("notes.zip");
    fs::write(&file, "just text").unwrap();

    let error = list_entries(&file).unwrap_err();

    assert!(matches!(error, Error::UnsupportedFormat { .. }), "{error}");
}
