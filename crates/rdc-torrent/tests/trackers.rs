use rdc_torrent::parse_tracker_list;

#[test]
fn filters_comments_trims_slashes_and_deduplicates_case_insensitively() {
    let text = "\r\n # ignored\n\t\nUDP://Tracker.Example:80/announce///\r\nudp://tracker.example:80/announce\nhttps://secure.example/\nwss://socket.example///\nhttp://under_score.example/announce\n";
    assert_eq!(
        parse_tracker_list(text),
        (
            vec![
                "UDP://Tracker.Example:80/announce".into(),
                "https://secure.example".into(),
                "wss://socket.example".into(),
                "http://under_score.example/announce".into()
            ],
            0
        )
    );
}

#[test]
fn rejects_invalid_schemes_hosts_and_dangerous_characters() {
    let rejected = [
        "ftp://tracker.example",
        "ws://tracker.example",
        "not a url",
        "udp:///announce",
        "https://example.com/../announce",
        "http://two..dots/announce",
        "http://tracker.example\\announce",
        "http://tracker.example/an\tnounce",
        "udp://bad!host/announce",
        "udp://[bad:ipv6]/announce",
        "http://tracker.example/\u{7f}",
    ];
    let (values, count) = parse_tracker_list(&rejected.join("\n"));
    assert_eq!(values, Vec::<String>::new());
    assert_eq!(count, rejected.len());
}

#[test]
fn accepts_ipv4_ipv6_and_preserves_trim_order() {
    let (values, count) = parse_tracker_list(
        "udp://127.0.0.1:80/announce\nhttps://[2001:db8::1]:443/announce\n https://example.com/ \n",
    );
    assert_eq!(count, 0);
    assert_eq!(
        values,
        [
            "udp://127.0.0.1:80/announce",
            "https://[2001:db8::1]:443/announce",
            "https://example.com/"
        ]
    );
}
