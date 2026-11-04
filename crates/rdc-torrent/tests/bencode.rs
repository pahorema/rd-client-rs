use rdc_torrent::bencode::{self, Limits, Value};

#[test]
fn preserves_raw_spans_and_sorts_keys() {
    let input = b"d1:zi-23e1:al0:3:\x00\xffxee";
    let node = bencode::decode(input).unwrap();
    assert_eq!(&input[node.get(b"z").unwrap().span.clone()], b"i-23e");
    let canonical = bencode::encode(&node);
    assert_eq!(canonical, b"d1:al0:3:\x00\xffxe1:zi-23ee");
    assert_eq!(
        bencode::encode(&bencode::decode(&canonical).unwrap()),
        canonical
    );
}

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

#[test]
fn random_bytes_never_panic() {
    let mut state = 17u64;
    for length in 0..512 {
        let mut bytes = vec![0; length];
        for byte in &mut bytes {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            *byte = state.to_le_bytes()[0];
        }
        if let Ok(node) = bencode::decode(&bytes) {
            let encoded = bencode::encode(&node);
            assert!(bencode::decode(&encoded).is_ok());
        }
    }
}
