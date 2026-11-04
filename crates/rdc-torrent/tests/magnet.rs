use rdc_torrent::{MagnetLink, ParseMagnetError};

#[test]
fn parses_hex_base32_display_name_and_ordered_trackers() {
    let hash = "0123456789abcdef0123456789abcdef01234567";
    let bytes = data_encoding::HEXLOWER.decode(hash.as_bytes()).unwrap();
    for topic in [
        hash.to_ascii_uppercase(),
        data_encoding::BASE32_NOPAD
            .encode(&bytes)
            .to_ascii_lowercase(),
    ] {
        let magnet = MagnetLink::parse(&format!("magnet:?xt=urn:btih:{topic}&dn=Film+%26+More%20%C3%A9&tr=udp%3A%2F%2Fone&tr=https%3A%2F%2Ftwo&x.a=1&x.a=2")).unwrap();
        assert_eq!(magnet.info_hash_hex(), hash);
        assert_eq!(magnet.display_name.as_deref(), Some("Film & More é"));
        assert_eq!(magnet.trackers, ["udp://one", "https://two"]);
        assert_eq!(
            magnet.other_params,
            [("x.a".into(), "1".into()), ("x.a".into(), "2".into())]
        );
    }
    let magnet = MagnetLink::parse(&format!("magnet:?xt=urn:btih:{hash}")).unwrap();
    assert_eq!(magnet.display_name, None);
    assert_eq!(magnet.trackers, Vec::<String>::new());
}

#[test]
fn parses_v2_and_hybrid_topics() {
    let v2 = "ab".repeat(32);
    let v1 = "cd".repeat(20);
    let parsed = MagnetLink::parse(&format!("magnet:?xt=urn:btmh:1220{v2}")).unwrap();
    assert_eq!(parsed.info_hash_hex(), v2);
    assert!(parsed.info_hash_v1.is_none());
    let hybrid = MagnetLink::parse(&format!(
        "magnet:?xt=urn:btmh:1220{v2}&xt=urn:btih:{v1}&xt=urn:other:value"
    ))
    .unwrap();
    assert_eq!(hybrid.info_hash_hex(), v1);
    assert_eq!(
        hybrid.other_params,
        [("xt".into(), "urn:other:value".into())]
    );
}

#[test]
fn rejects_missing_malformed_and_conflicting_hashes() {
    for value in [
        "https://example.com",
        "magnet:?dn=nohash",
        "magnet:?xt=urn:btih:xxx",
        "magnet:?xt=urn:btih:GGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGG",
        "magnet:?xt=urn:btmh:1220abcd",
        "magnet:?xt=urn:btmh:1320abcd",
    ] {
        assert!(MagnetLink::parse(value).is_err(), "{value}");
    }
    let value = format!(
        "magnet:?xt=urn:btih:{}&xt=urn:btih:{}",
        "a".repeat(40),
        "b".repeat(40)
    );
    assert_eq!(
        MagnetLink::parse(&value),
        Err(ParseMagnetError::ConflictingHashes)
    );
}
