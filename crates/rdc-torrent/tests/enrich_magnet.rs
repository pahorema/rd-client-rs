use rdc_torrent::enrich_magnet;
use std::borrow::Cow;

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|s| (*s).to_owned()).collect()
}
const MAGNET: &str = "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567&dn=TestFile&tr=http%3A%2F%2Ftracker1.com%2Fannounce&tr=http%3A%2F%2Ftracker2.com%2Fannounce";

#[test]
fn no_trackers_returns_exact_input() {
    assert!(matches!(enrich_magnet(MAGNET, &[]), Cow::Borrowed(_)));
    assert_eq!(enrich_magnet(MAGNET, &[]), MAGNET);
}

#[test]
fn provided_trackers_are_appended_in_order() {
    let cases: &[(&str, &[&str], &str)] = &[
        (
            "magnet:?xt=urn:btih:HASH",
            &["http://new-tracker.com/announce"],
            "magnet:?xt=urn:btih:HASH&tr=http%3A%2F%2Fnew-tracker.com%2Fannounce",
        ),
        (
            "magnet:?xt=urn:btih:HASH",
            &[
                "http://tracker1.com/announce",
                "http://tracker2.com/announce",
            ],
            "magnet:?xt=urn:btih:HASH&tr=http%3A%2F%2Ftracker1.com%2Fannounce&tr=http%3A%2F%2Ftracker2.com%2Fannounce",
        ),
        (
            "magnet:?xt=urn:btih:HASH",
            &["udp://tracker1"],
            "magnet:?xt=urn:btih:HASH&tr=udp%3A%2F%2Ftracker1",
        ),
        (
            "magnet:?xt=urn:btih:HASH",
            &["udp://tracker1", "udp://tracker2"],
            "magnet:?xt=urn:btih:HASH&tr=udp%3A%2F%2Ftracker1&tr=udp%3A%2F%2Ftracker2",
        ),
        ("magnet:?xt=urn:btih:HASH", &[], "magnet:?xt=urn:btih:HASH"),
        (
            "magnet:?",
            &["udp://tracker1"],
            "magnet:?tr=udp%3A%2F%2Ftracker1",
        ),
        (
            "magnet:?",
            &["udp://tracker1", "udp://tracker2"],
            "magnet:?tr=udp%3A%2F%2Ftracker1&tr=udp%3A%2F%2Ftracker2",
        ),
        ("magnet:?", &[], "magnet:?"),
        (
            "magnet:",
            &["udp://tracker1"],
            "magnet:?tr=udp%3A%2F%2Ftracker1",
        ),
    ];
    for (input, trackers, expected) in cases {
        assert_eq!(enrich_magnet(input, &strings(trackers)), *expected);
    }
}

#[test]
fn duplicates_add_only_unique_trackers() {
    assert_eq!(
        enrich_magnet(
            MAGNET,
            &strings(&[
                "http://new-tracker.com/announce",
                "http://tracker1.com/announce"
            ])
        ),
        format!("{MAGNET}&tr=http%3A%2F%2Fnew-tracker.com%2Fannounce")
    );
    assert_eq!(
        enrich_magnet(MAGNET, &strings(&["HTTP://TRACKER1.COM/announce"])),
        MAGNET
    );
}

#[test]
fn preserves_parameters_and_includes_all_trackers() {
    let input = "magnet:?xt=urn:btih:HASH&dn=MyFile&tr=udp%3A%2F%2Fexisting";
    let output = enrich_magnet(
        input,
        &strings(&["udp://new1", "udp://existing", "UDP://NEW1"]),
    );
    let pairs: Vec<_> =
        url::form_urlencoded::parse(output.split_once('?').unwrap().1.as_bytes()).collect();
    assert_eq!(
        pairs[0],
        (Cow::Borrowed("xt"), Cow::Borrowed("urn:btih:HASH"))
    );
    assert_eq!(pairs[1], (Cow::Borrowed("dn"), Cow::Borrowed("MyFile")));
    assert_eq!(
        pairs
            .iter()
            .filter(|(k, _)| k == "tr")
            .map(|(_, v)| v.as_ref())
            .collect::<Vec<_>>(),
        ["udp://existing", "udp://new1"]
    );
}

#[test]
fn existing_parameters_are_kept_byte_for_byte() {
    let input =
        "magnet:?xt=urn%3Abtih%3AHASH&dn=Name+%26+%C3%A9&x=a%2Bb%26c&x=d%3De&TR=udp%3A%2F%2Fone";
    assert_eq!(
        enrich_magnet(input, &strings(&["udp://two", "udp://one"])),
        format!("{input}&tr=udp%3A%2F%2Ftwo")
    );
    assert_eq!(
        enrich_magnet(
            "magnet:?xt=urn:btih:HASH&flag&dn=a+b&tr=udp%3A%2F%2Fone#frag",
            &strings(&["udp://two"])
        ),
        "magnet:?xt=urn:btih:HASH&flag&dn=a+b&tr=udp%3A%2F%2Fone&tr=udp%3A%2F%2Ftwo#frag"
    );
    assert_eq!(
        enrich_magnet("magnet:?xt=urn:btih:HASH&", &strings(&["udp://two"])),
        "magnet:?xt=urn:btih:HASH&tr=udp%3A%2F%2Ftwo"
    );
}

#[test]
fn trackers_supplied_twice_are_added_once() {
    for input in ["magnet:", "magnet:?"] {
        assert_eq!(
            enrich_magnet(input, &strings(&["udp://one", "UDP://ONE"])),
            "magnet:?tr=udp%3A%2F%2Fone"
        );
    }
}
