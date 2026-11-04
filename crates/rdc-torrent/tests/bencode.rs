use rdc_torrent::bencode::{self, Limits, Value};

#[test]
fn rejects_malformed_tokens_with_offsets() {
    for input in [
        b"".as_slice(),
        b"i-e",
        b"i+1e",
        b"i01e",
        b"i-0e",
        b"i9223372036854775808e",
        b"01:a",
        b"9:a",
        b"999999999999999999999999999999999:a",
        b"d1:ai1e1:ai2ee",
        b"di1ei2ee",
        b"l",
        b"i1ex",
    ] {
        let error = bencode::decode(input).unwrap_err();
        assert!(error.offset <= input.len(), "{input:?}: {error}");
    }
    assert!(matches!(
        bencode::decode(b"i-9223372036854775808e").unwrap().value,
        Value::Integer(i64::MIN)
    ));
}

#[test]
fn bounds_depth_size_and_node_count() {
    for limits in [
        Limits {
            max_bytes: 2,
            ..Limits::default()
        },
        Limits {
            max_depth: 0,
            ..Limits::default()
        },
        Limits {
            max_nodes: 1,
            ..Limits::default()
        },
    ] {
        assert!(bencode::decode_with_limits(b"li1ee", limits).is_err());
    }
    let nested = [vec![b'l'; 1000], vec![b'e'; 1000]].concat();
    assert!(bencode::decode(&nested).is_err());
}
