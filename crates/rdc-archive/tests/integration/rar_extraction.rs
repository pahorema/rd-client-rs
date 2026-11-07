use std::fs;

use rdc_archive::{Error, extract, list_entries};
use tokio_util::sync::CancellationToken;

use crate::common::{big_text, copy_fixtures, copy_fixtures_as, files_below};
use crate::rar5::{Rar5, Redirect};

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

#[test]
fn large_rar_entries_are_extracted_and_complete_the_progress() {
    let dir = tempfile::tempdir().unwrap();
    let archive = copy_fixtures(dir.path(), &["zeros.rar"]);
    let destination = dir.path().join("out");
    let mut seen = Vec::new();

    extract(
        &archive,
        &destination,
        |percent| seen.push(percent),
        &CancellationToken::new(),
    )
    .unwrap();

    let zeros = fs::read(destination.join("zeros.bin")).unwrap();
    assert_eq!(zeros.len(), 9 * 1024 * 1024);
    assert!(zeros.iter().all(|byte| *byte == 0));
    assert_eq!(seen.last(), Some(&100));
    assert!(seen.windows(2).all(|pair| pair[0] < pair[1]), "{seen:?}");
}

#[test]
fn stored_rar5_entries_built_by_the_tests_are_extracted() {
    let dir = tempfile::tempdir().unwrap();
    let archive = dir.path().join("built.rar");
    Rar5::new()
        .file("a.txt", b"alpha")
        .file("dir/b.txt", b"beta")
        .write(&archive);
    let destination = dir.path().join("out");

    extract_all(&archive, &destination).unwrap();

    assert_eq!(files_below(&destination), ["a.txt", "dir/b.txt"]);
    assert_eq!(
        fs::read_to_string(destination.join("dir/b.txt")).unwrap(),
        "beta"
    );
}

#[test]
fn rar_link_and_file_copy_entries_are_refused_before_anything_is_written() {
    for kind in [
        Redirect::HardLink,
        Redirect::FileCopy,
        Redirect::UnixSymlink,
        Redirect::WindowsSymlink,
        Redirect::Junction,
    ] {
        let dir = tempfile::tempdir().unwrap();
        let secret = dir.path().join("session.key");
        fs::write(&secret, "secret").unwrap();
        let archive = dir.path().join("links.rar");
        Rar5::new()
            .file("readme.txt", b"harmless")
            .redirect("Movie.mkv", kind, &secret.to_string_lossy())
            .write(&archive);
        let destination = dir.path().join("out");

        let error = extract_all(&archive, &destination).unwrap_err();

        assert!(
            matches!(&error, Error::UnsupportedEntry { entry, .. } if entry == "Movie.mkv"),
            "{kind:?}: {error}"
        );
        assert_eq!(files_below(&destination), Vec::<String>::new(), "{kind:?}");
        assert_eq!(fs::read_to_string(&secret).unwrap(), "secret", "{kind:?}");
    }
}

#[test]
fn a_refused_link_names_the_entry_and_its_kind() {
    let dir = tempfile::tempdir().unwrap();
    let archive = dir.path().join("link.rar");
    Rar5::new()
        .redirect("Movie.mkv", Redirect::HardLink, "data/db/rd-client.db")
        .write(&archive);

    let error = extract_all(&archive, &dir.path().join("out")).unwrap_err();

    assert_eq!(
        error.to_string(),
        "archive entry \"Movie.mkv\" cannot be extracted: it is a hard link"
    );
}
