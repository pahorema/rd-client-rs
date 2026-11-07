use rdc_torrent::{
    ParseTorrentError, TorrentFile,
    bencode::{self, Node, Value},
};
use sha1::{Digest, Sha1};

fn string(s: &'static [u8]) -> Node<'static> {
    Node::new(Value::Bytes(s))
}
fn int(n: i64) -> Node<'static> {
    Node::new(Value::Integer(n))
}
fn dict(entries: Vec<(&'static [u8], Node<'static>)>) -> Node<'static> {
    Node::new(Value::Dictionary(entries.into_iter().collect()))
}
fn list(items: Vec<Node<'static>>) -> Node<'static> {
    Node::new(Value::List(items))
}
fn single() -> Node<'static> {
    dict(vec![
        (b"name", string(b"film.mkv")),
        (b"length", int(42)),
        (b"piece length", int(16384)),
        (b"pieces", string(b"01234567890123456789")),
        (b"source", string(b"Publisher")),
    ])
}
fn torrent(info: Node<'static>) -> Vec<u8> {
    bencode::encode(&dict(vec![
        (b"info", info),
        (b"announce", string(b"udp://tracker")),
        (
            b"announce-list",
            list(vec![list(vec![
                string(b"udp://tier1"),
                string(b"https://tier2"),
            ])]),
        ),
    ]))
}

#[test]
fn parses_single_file_with_source_trackers_and_raw_hash() {
    let bytes = torrent(single());
    let parsed = TorrentFile::parse(&bytes).unwrap();
    assert_eq!(parsed.name, "film.mkv");
    assert_eq!(parsed.total_size, 42);
    assert_eq!(parsed.files[0].path, ["film.mkv"]);
    assert_eq!(parsed.source.as_deref(), Some("Publisher"));
    assert_eq!(parsed.announce.as_deref(), Some("udp://tracker"));
    assert_eq!(parsed.announce_list, [vec!["udp://tier1", "https://tier2"]]);
    let root = bencode::decode(&bytes).unwrap();
    let hash: [u8; 20] = Sha1::digest(&bytes[root.get(b"info").unwrap().span.clone()]).into();
    assert_eq!(parsed.info_hash_v1, Some(hash));
    assert_eq!(
        parsed.info_hash_hex(),
        data_encoding::HEXLOWER.encode(&hash)
    );
    let unsorted = b"d4:infod4:name1:x6:lengthi1eee";
    let parsed = TorrentFile::parse(unsorted).unwrap();
    assert_eq!(
        parsed.info_hash_hex(),
        data_encoding::HEXLOWER.encode(&Sha1::digest(b"d4:name1:x6:lengthi1ee"))
    );
}

#[test]
fn parses_multiple_files_and_checked_total_size() {
    let file = |name, n| {
        dict(vec![
            (b"path", list(vec![string(b"season"), string(name)])),
            (b"length", int(n)),
        ])
    };
    let bytes = torrent(dict(vec![
        (b"name", string(b"show")),
        (b"files", list(vec![file(b"1.mkv", 7), file(b"2.mkv", 12)])),
    ]));
    let parsed = TorrentFile::parse(&bytes).unwrap();
    assert_eq!(parsed.total_size, 19);
    assert_eq!(parsed.files[1].path, ["season", "2.mkv"]);
    assert!(matches!(
        TorrentFile::parse(&torrent(dict(vec![
            (b"name", string(b"x")),
            (b"length", int(-1))
        ]))),
        Err(ParseTorrentError::Invalid("length"))
    ));
    for bytes in [b"de".as_slice(), b"le", b"d4:infoi1ee", b"d4:infodee"] {
        assert!(TorrentFile::parse(bytes).is_err());
    }
}

#[test]
fn parses_v2_tree_and_hybrid_hashes() {
    use sha2::Sha256;
    for hybrid in [false, true] {
        let mut info = dict(vec![
            (b"name", string(b"collection")),
            (b"meta version", int(2)),
            (b"piece length", int(16384)),
            (
                b"file tree",
                dict(vec![
                    (
                        b"folder",
                        dict(vec![(
                            b"film.mkv",
                            dict(vec![(
                                b"",
                                dict(vec![
                                    (b"length", int(2048)),
                                    (b"pieces root", string(b"01234567890123456789012345678901")),
                                ]),
                            )]),
                        )]),
                    ),
                    (
                        b"empty.txt",
                        dict(vec![(b"", dict(vec![(b"length", int(0))]))]),
                    ),
                ]),
            ),
        ]);
        if hybrid {
            let Value::Dictionary(entries) = &mut info.value else {
                unreachable!()
            };
            entries.insert(b"pieces", string(b"01234567890123456789"));
            entries.insert(
                b"files",
                list(vec![dict(vec![
                    (b"path", list(vec![string(b"folder"), string(b"film.mkv")])),
                    (b"length", int(2048)),
                ])]),
            );
        }
        let bytes = torrent(info);
        let root = bencode::decode(&bytes).unwrap();
        let raw = &bytes[root.get(b"info").unwrap().span.clone()];
        let parsed = TorrentFile::parse(&bytes).unwrap();
        assert_eq!(parsed.files.len(), 2);
        assert_eq!(parsed.files[1].path, ["folder", "film.mkv"]);
        assert_eq!(parsed.total_size, 2048);
        assert_eq!(parsed.info_hash_v2, Some(Sha256::digest(raw).into()));
        assert_eq!(parsed.info_hash_v1.is_some(), hybrid);
        let expected = if hybrid {
            data_encoding::HEXLOWER.encode(&Sha1::digest(raw))
        } else {
            data_encoding::HEXLOWER.encode(&Sha256::digest(raw))
        };
        assert_eq!(parsed.info_hash_hex(), expected);
    }
}

#[test]
fn rejects_malformed_v2_trees_and_total_overflow() {
    for tree in [
        int(2),
        dict(vec![]),
        dict(vec![(b"", dict(vec![(b"length", int(1))]))]),
        dict(vec![(
            b"x",
            dict(vec![(b"", dict(vec![(b"length", int(-1))]))]),
        )]),
    ] {
        assert!(
            TorrentFile::parse(&torrent(dict(vec![
                (b"name", string(b"x")),
                (b"meta version", int(2)),
                (b"file tree", tree)
            ])))
            .is_err()
        );
    }
    let file = || {
        dict(vec![
            (b"path", list(vec![string(b"x")])),
            (b"length", int(i64::MAX)),
        ])
    };
    assert_eq!(
        TorrentFile::parse(&torrent(dict(vec![
            (b"name", string(b"x")),
            (b"files", list(vec![file(), file(), file()]))
        ]))),
        Err(ParseTorrentError::SizeOverflow)
    );
}

#[test]
fn prefers_the_utf8_name_and_decodes_other_text_leniently() {
    let bytes = b"d8:announce5:udp:\xff4:infod6:lengthi1e4:name4:caf\xe910:name.utf-85:caf\xc3\xa96:source3:x\xffyee";
    let parsed = TorrentFile::parse(bytes).unwrap();
    assert_eq!(parsed.name, "caf\u{e9}");
    assert_eq!(parsed.files[0].path, ["caf\u{e9}"]);
    assert_eq!(parsed.source.as_deref(), Some("x\u{fffd}y"));
    assert_eq!(parsed.announce.as_deref(), Some("udp:\u{fffd}"));

    let bytes = b"d4:infod5:filesld6:lengthi1e4:pathl2:a\xffeee4:name2:n\xffee";
    let parsed = TorrentFile::parse(bytes).unwrap();
    assert_eq!(parsed.name, "n\u{fffd}");
    assert_eq!(parsed.files[0].path, ["a\u{fffd}"]);
}
