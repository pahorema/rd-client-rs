use rdc_torrent::{
    EnrichTorrentError,
    bencode::{self, Node, Value},
    enrich_torrent,
};
use std::{borrow::Cow, collections::BTreeMap};

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|s| (*s).to_owned()).collect()
}
fn string(s: &[u8]) -> Node<'_> {
    Node::new(Value::Bytes(s))
}
fn fixture(primary: Option<&str>, tiers: &[Vec<&str>]) -> Vec<u8> {
    let mut entries = BTreeMap::new();
    entries.insert(
        b"info".as_slice(),
        Node::new(Value::Dictionary(BTreeMap::new())),
    );
    if let Some(primary) = primary {
        entries.insert(b"announce", string(primary.as_bytes()));
    }
    if !tiers.is_empty() {
        entries.insert(
            b"announce-list",
            Node::new(Value::List(
                tiers
                    .iter()
                    .map(|tier| {
                        Node::new(Value::List(
                            tier.iter().map(|s| string(s.as_bytes())).collect(),
                        ))
                    })
                    .collect(),
            )),
        );
    }
    bencode::encode(&Node::new(Value::Dictionary(entries)))
}
fn flattened<'src>(root: &Node<'src>) -> Vec<&'src [u8]> {
    root.get(b"announce-list")
        .expect("enrichment creates announce-list")
        .list()
        .expect("announce-list is a list")
        .iter()
        .flat_map(|tier| {
            let tier = tier.list().expect("each tier is a list");
            assert_eq!(tier.len(), 1);
            tier.iter()
                .map(|n| n.bytes().expect("trackers are strings"))
        })
        .collect()
}

#[test]
fn added_trackers_create_both_keys_include_all_and_set_primary() {
    let bytes = fixture(
        Some("http://tracker1.com/announce"),
        &[vec!["http://tracker1.com/announce"]],
    );
    let output = enrich_torrent(&bytes, &strings(&["http://new-tracker.com/announce"])).unwrap();
    let root = bencode::decode(&output).unwrap();
    let trackers = flattened(&root);
    assert_eq!(
        trackers,
        [
            b"http://tracker1.com/announce".as_slice(),
            b"http://new-tracker.com/announce"
        ]
    );
    assert_eq!(root.get(b"announce").unwrap().bytes(), Some(trackers[0]));
}

#[test]
fn duplicates_are_unique_and_no_additions_keep_identical_bytes() {
    let bytes = fixture(
        Some("http://tracker1.com/announce"),
        &[vec!["http://tracker1.com/announce"]],
    );
    let output = enrich_torrent(&bytes, &strings(&["HTTP://TRACKER1.COM/announce"])).unwrap();
    assert_eq!(output, bytes);
    let root = bencode::decode(&output).unwrap();
    assert_eq!(
        flattened(&root),
        [b"http://tracker1.com/announce".as_slice()]
    );
    assert!(matches!(output, Cow::Borrowed(_)));
}

#[test]
fn missing_announce_list_keeps_primary_and_adds_grabbed_trackers() {
    let bytes = fixture(Some("http://tracker1.com/announce"), &[]);
    let output = enrich_torrent(&bytes, &strings(&["http://new-tracker.com/announce"])).unwrap();
    let root = bencode::decode(&output).unwrap();
    assert_eq!(
        flattened(&root),
        [
            b"http://tracker1.com/announce".as_slice(),
            b"http://new-tracker.com/announce"
        ]
    );
}

#[test]
fn no_new_trackers_returns_exact_bytes_with_and_without_announce_fields() {
    for bytes in [
        fixture(
            Some("http://existing.tracker/announce"),
            &[vec!["http://existing.tracker/announce"]],
        ),
        fixture(None, &[]),
        b"d4:infode8:announce3:urle".to_vec(),
    ] {
        assert_eq!(enrich_torrent(&bytes, &[]).unwrap(), bytes);
    }
    let bytes = fixture(None, &[]);
    let output = enrich_torrent(&bytes, &[]).unwrap();
    let root = bencode::decode(&output).unwrap();
    assert_eq!(root.get(b"announce"), None);
    assert_eq!(root.get(b"announce-list"), None);
}

#[test]
fn rejects_empty_invalid_and_non_dictionary_input_before_enrichment() {
    assert_eq!(enrich_torrent(&[], &[]), Err(EnrichTorrentError::Empty));
    assert!(matches!(
        enrich_torrent(&[1, 2, 3, 4, 5], &[]),
        Err(EnrichTorrentError::Bencode(_))
    ));
    assert_eq!(
        enrich_torrent(b"le", &strings(&["udp://one"])),
        Err(EnrichTorrentError::NotDictionary)
    );
}

#[test]
fn missing_announce_fields_create_primary_and_ordered_singleton_tiers() {
    let bytes = fixture(None, &[]);
    let output = enrich_torrent(&bytes, &strings(&["udp://tracker1", "udp://tracker2"])).unwrap();
    let root = bencode::decode(&output).unwrap();
    assert_eq!(
        root.get(b"announce").unwrap().bytes(),
        Some(b"udp://tracker1".as_slice())
    );
    assert_eq!(
        flattened(&root),
        [b"udp://tracker1".as_slice(), b"udp://tracker2"]
    );
}

#[test]
fn merges_existing_tiers_before_primary_and_new_trackers() {
    let bytes = fixture(
        Some("http://existing.com/announce"),
        &[
            vec!["http://tier1.com"],
            vec!["http://existing.com/announce"],
        ],
    );
    let output = enrich_torrent(
        &bytes,
        &strings(&["udp://new1", "http://existing.com/announce"]),
    )
    .unwrap();
    let root = bencode::decode(&output).unwrap();
    assert_eq!(
        flattened(&root),
        [
            b"http://tier1.com".as_slice(),
            b"http://existing.com/announce",
            b"udp://new1"
        ]
    );
    assert_eq!(
        root.get(b"announce").unwrap().bytes(),
        Some(b"http://tier1.com".as_slice())
    );
}

#[test]
fn ignores_non_strings_and_non_list_tiers_and_keeps_info_structure() {
    let bytes = b"d13:announce-listli1el3:urli2eee4:infod4:name1:x6:lengthi1eee";
    let output = enrich_torrent(bytes, &strings(&["udp://new"])).unwrap();
    let before = bencode::decode(bytes).unwrap();
    let after = bencode::decode(&output).unwrap();
    assert_eq!(
        bencode::encode(before.get(b"info").unwrap()),
        bencode::encode(after.get(b"info").unwrap())
    );
    assert_eq!(flattened(&after), [b"url".as_slice(), b"udp://new"]);
}
