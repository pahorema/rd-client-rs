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
