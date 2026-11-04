use rdc_torrent::{MagnetLink, TorrentFile, banned_magnet_error, banned_torrent_error};

fn magnet() -> MagnetLink {
    MagnetLink::parse("magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567&tr=+UDP%3A%2F%2FBANNED.example%2Fa+&tr=https%3A%2F%2Fbanned.example%2Fb&tr=udp%3A%2F%2Fsafe").expect("valid fixture")
}
fn torrent() -> TorrentFile {
    TorrentFile {
        name: "file".into(),
        info_hash_v1: Some([0; 20]),
        info_hash_v2: None,
        announce: Some("udp://primary".into()),
        announce_list: vec![
            vec!["udp://safe".into(), "udp://BANNED.one".into()],
            vec!["https://banned.two".into()],
        ],
        source: Some("CaseSensitivePublisher".into()),
        files: vec![],
        total_size: 0,
    }
}

#[test]
fn matches_magnet_substrings_in_order_and_preserves_visible_urls() {
    let magnet = magnet();
    assert_eq!(
        banned_magnet_error(" , missing, BANNED , safe", &magnet).as_deref(),
        Some(
            "Cannot add torrent, the torrent contains banned trackers:  UDP://BANNED.example/a , https://banned.example/b."
        )
    );
    assert_eq!(banned_magnet_error(" , missing, ", &magnet), None);
    assert_eq!(
        banned_magnet_error("safe,banned", &magnet).as_deref(),
        Some("Cannot add torrent, the torrent contains banned trackers: udp://safe.")
    );
}

#[test]
fn source_matching_is_raw_case_sensitive_and_precedes_urls_per_entry() {
    let file = torrent();
    assert_eq!(
        banned_torrent_error("Publisher", &file).as_deref(),
        Some(
            "Cannot add torrent, the torrent source 'CaseSensitivePublisher' is a banned tracker."
        )
    );
    assert_eq!(banned_torrent_error("publisher", &file), None);
    assert_eq!(banned_torrent_error(" Publisher ", &file), None);
    assert_eq!(
        banned_torrent_error(" ,CaseSensitivePublisher,banned", &file).as_deref(),
        Some(
            "Cannot add torrent, the torrent source 'CaseSensitivePublisher' is a banned tracker."
        )
    );
    assert_eq!(
        banned_torrent_error("banned,Publisher", &file).as_deref(),
        Some(
            "Cannot add torrent, the torrent contains banned trackers: udp://BANNED.one, https://banned.two."
        )
    );
}

#[test]
fn file_checks_all_tiers_and_uses_primary_only_without_tiers() {
    let mut file = torrent();
    assert_eq!(
        banned_torrent_error(" BANNED ", &file).as_deref(),
        Some(
            "Cannot add torrent, the torrent contains banned trackers: udp://BANNED.one, https://banned.two."
        )
    );
    assert_eq!(banned_torrent_error("primary", &file), None);
    file.announce_list.clear();
    assert_eq!(
        banned_torrent_error("primary", &file).as_deref(),
        Some("Cannot add torrent, the torrent contains banned trackers: udp://primary.")
    );
}
