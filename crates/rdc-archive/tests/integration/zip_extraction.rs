use std::fs;

use rdc_archive::{Error, extract, list_entries};
use tokio_util::sync::CancellationToken;

use crate::common::{files_below, write_zip, write_zip_declaring};

#[test]
fn zip_with_nested_folders_is_extracted() {
    let dir = tempfile::tempdir().unwrap();
    let archive = dir.path().join("archive.zip");
    write_zip(
        &archive,
        &[
            ("top.txt", b"top"),
            ("folder/", b""),
            ("folder/a.txt", b"alpha"),
            ("folder/sub/b.txt", b"beta"),
            ("empty/", b""),
        ],
    );
    let destination = dir.path().join("out");

    extract(&archive, &destination, |_| {}, &CancellationToken::new()).unwrap();

    assert_eq!(
        files_below(&destination),
        ["folder/a.txt", "folder/sub/b.txt", "top.txt"]
    );
    assert_eq!(
        fs::read_to_string(destination.join("folder/sub/b.txt")).unwrap(),
        "beta"
    );
    assert!(destination.join("empty").is_dir());
}

#[test]
fn backslash_separated_names_create_directories() {
    let dir = tempfile::tempdir().unwrap();
    let archive = dir.path().join("archive.zip");
    write_zip(&archive, &[("folder\\file.txt", b"x")]);
    let destination = dir.path().join("out");

    extract(&archive, &destination, |_| {}, &CancellationToken::new()).unwrap();

    assert_eq!(files_below(&destination), ["folder/file.txt"]);
}

#[test]
fn existing_files_are_overwritten_and_truncated() {
    let dir = tempfile::tempdir().unwrap();
    let archive = dir.path().join("archive.zip");
    write_zip(&archive, &[("file.txt", b"new")]);
    let destination = dir.path().join("out");
    fs::create_dir_all(&destination).unwrap();
    fs::write(destination.join("file.txt"), "a much longer old content").unwrap();

    extract(&archive, &destination, |_| {}, &CancellationToken::new()).unwrap();

    assert_eq!(
        fs::read_to_string(destination.join("file.txt")).unwrap(),
        "new"
    );
}

#[test]
fn progress_is_increasing_and_reaches_one_hundred() {
    let dir = tempfile::tempdir().unwrap();
    let archive = dir.path().join("archive.zip");
    let large = vec![7_u8; 3 * 1024 * 1024];
    write_zip(&archive, &[("small.bin", b"abc"), ("large.bin", &large)]);
    let mut seen = Vec::new();

    extract(
        &archive,
        &dir.path().join("out"),
        |percent| seen.push(percent),
        &CancellationToken::new(),
    )
    .unwrap();

    assert_eq!(seen.first(), Some(&0));
    assert_eq!(seen.last(), Some(&100));
    assert!(seen.len() > 3, "intermediate values reported: {seen:?}");
    assert!(seen.windows(2).all(|pair| pair[0] < pair[1]), "{seen:?}");
}

#[test]
fn cancellation_stops_a_large_entry_mid_write() {
    let dir = tempfile::tempdir().unwrap();
    let archive = dir.path().join("archive.zip");
    let large = vec![1_u8; 4 * 1024 * 1024];
    write_zip(&archive, &[("large.bin", &large), ("after.txt", b"x")]);
    let destination = dir.path().join("out");
    let cancel = CancellationToken::new();

    let result = extract(
        &archive,
        &destination,
        |percent| {
            if percent >= 10 {
                cancel.cancel();
            }
        },
        &cancel,
    );

    assert!(matches!(result, Err(Error::Cancelled)), "{result:?}");
    let written = fs::metadata(destination.join("large.bin")).unwrap().len();
    assert!(written < 4 * 1024 * 1024, "{written} bytes written");
    assert!(!destination.join("after.txt").exists());
}

#[test]
fn an_already_cancelled_token_extracts_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let archive = dir.path().join("archive.zip");
    write_zip(&archive, &[("file.txt", b"x")]);
    let destination = dir.path().join("out");
    let cancel = CancellationToken::new();
    cancel.cancel();

    let result = extract(&archive, &destination, |_| {}, &cancel);

    assert!(matches!(result, Err(Error::Cancelled)), "{result:?}");
    assert_eq!(files_below(&destination), Vec::<String>::new());
}

#[test]
fn entries_escaping_the_destination_are_rejected_before_writing() {
    let dir = tempfile::tempdir().unwrap();
    let archive = dir.path().join("archive.zip");
    write_zip(
        &archive,
        &[("good.txt", b"fine"), ("../evil.txt", b"gotcha")],
    );
    let destination = dir.path().join("out");

    let error = extract(&archive, &destination, |_| {}, &CancellationToken::new()).unwrap_err();

    assert!(
        matches!(&error, Error::UnsafeEntry { entry } if entry == "../evil.txt"),
        "{error}"
    );
    assert!(error.to_string().contains("../evil.txt"));
    assert!(!dir.path().join("evil.txt").exists());
    assert!(!destination.join("good.txt").exists());
}

#[test]
fn absolute_entry_names_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let archive = dir.path().join("archive.zip");
    write_zip(&archive, &[("/tmp/evil.txt", b"gotcha")]);

    let error = extract(
        &archive,
        &dir.path().join("out"),
        |_| {},
        &CancellationToken::new(),
    )
    .unwrap_err();

    assert!(matches!(error, Error::UnsafeEntry { .. }), "{error}");
}

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

#[test]
fn damaged_archives_are_reported_as_unreadable() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("broken.zip");
    fs::write(&file, b"PK\x03\x04 not really a zip").unwrap();

    let error = extract(
        &file,
        &dir.path().join("out"),
        |_| {},
        &CancellationToken::new(),
    )
    .unwrap_err();

    assert!(matches!(error, Error::Read { .. }), "{error}");
}

#[test]
fn a_zip_declaring_far_more_data_than_its_size_is_refused_before_writing() {
    let dir = tempfile::tempdir().unwrap();
    let archive = dir.path().join("bomb.zip");
    write_zip_declaring(&archive, "bomb.bin", b"tiny", 0xF000_0000);
    let destination = dir.path().join("out");

    let error = extract(&archive, &destination, |_| {}, &CancellationToken::new()).unwrap_err();

    assert!(
        matches!(error, Error::TooLarge { unpacked: 0xF000_0000, limit } if limit == 1 << 30),
        "{error}"
    );
    assert!(!destination.exists());
}
