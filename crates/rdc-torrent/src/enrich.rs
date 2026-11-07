use crate::bencode::{self, Node, Value};
use std::{borrow::Cow, collections::HashSet};
use thiserror::Error;

/// Adds unique trackers and groups existing parameters in first-key order.
/// Values use percent encoding (including display names); exact topics retain readable colons.
/// Returns the exact input when no tracker is added. Fetch failures are handled by the caller.
#[must_use]
pub fn enrich_magnet<'src>(magnet: &'src str, trackers: &[String]) -> Cow<'src, str> {
    if trackers.is_empty() {
        return Cow::Borrowed(magnet);
    }
    let (scheme, query) = magnet.split_once('?').unwrap_or((magnet, ""));
    if query.is_empty() {
        let mut out = format!("{scheme}?");
        for tracker in trackers {
            append_parameter(&mut out, "tr", tracker);
        }
        return Cow::Owned(out);
    }
    let mut groups: Vec<(String, Vec<String>)> = Vec::new();
    for pair in query.split('&').filter(|s| s.contains('=')) {
        for (key, value) in url::form_urlencoded::parse(pair.as_bytes()) {
            if let Some((_, values)) = groups
                .iter_mut()
                .find(|(k, _)| k.eq_ignore_ascii_case(&key))
            {
                values.push(value.into_owned());
            } else {
                groups.push((key.into_owned(), vec![value.into_owned()]));
            }
        }
    }
    let mut existing = Vec::new();
    let mut seen = HashSet::new();
    if let Some(index) = groups
        .iter()
        .position(|(k, _)| k.eq_ignore_ascii_case("tr"))
    {
        for tracker in groups.remove(index).1 {
            push_unique(&mut existing, &mut seen, tracker);
        }
    }
    let count = existing.len();
    for tracker in trackers {
        if seen.insert(tracker.to_lowercase()) {
            existing.push(tracker.clone());
        }
    }
    if count == existing.len() {
        return Cow::Borrowed(magnet);
    }
    let mut out = format!("{scheme}?");
    for (key, values) in groups {
        for value in values {
            append_parameter(&mut out, &key, &value);
        }
    }
    for tracker in existing {
        append_parameter(&mut out, "tr", &tracker);
    }
    Cow::Owned(out)
}

fn append_parameter(out: &mut String, key: &str, value: &str) {
    if !out.ends_with('?') {
        out.push('&');
    }
    escape_into(key, out, false);
    out.push('=');
    escape_into(value, out, key.eq_ignore_ascii_case("xt"));
}
fn escape_into(value: &str, out: &mut String, topic: bool) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) || (topic && byte == b':') {
            out.push(char::from(byte));
        } else {
            out.push('%');
            out.push(char::from(HEX[usize::from(byte >> 4)]));
            out.push(char::from(HEX[usize::from(byte & 15)]));
        }
    }
}
fn push_unique(items: &mut Vec<String>, seen: &mut HashSet<String>, value: String) {
    if seen.insert(value.to_lowercase()) {
        items.push(value);
    }
}

/// Invalid input for torrent tracker enrichment.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum EnrichTorrentError {
    /// Empty torrent data cannot be decoded.
    #[error("torrent bytes cannot be empty")]
    Empty,
    /// The payload is malformed bencode.
    #[error("invalid torrent file format: {0}")]
    Bencode(#[from] bencode::DecodeError),
    /// The root is not a bencoded dictionary.
    #[error("invalid torrent file format: expected a dictionary")]
    NotDictionary,
}

/// Merges tier trackers, the primary announce, and new trackers, in that order.
/// Changed output has one tracker per tier; the root dictionary is re-encoded with sorted keys
/// while every other value keeps its exact bytes, so the info-hash does not change.
/// No additions return byte-identical borrowed input, even with noncanonical encoding.
pub fn enrich_torrent<'src>(
    bytes: &'src [u8],
    trackers: &[String],
) -> Result<Cow<'src, [u8]>, EnrichTorrentError> {
    if bytes.is_empty() {
        return Err(EnrichTorrentError::Empty);
    }
    let mut root = bencode::decode(bytes)?;
    let Value::Dictionary(entries) = &mut root.value else {
        return Err(EnrichTorrentError::NotDictionary);
    };
    if trackers.is_empty() {
        return Ok(Cow::Borrowed(bytes));
    }
    let mut all = Vec::new();
    let mut seen = HashSet::new();
    if let Some(tiers) = entries
        .get(b"announce-list".as_slice())
        .and_then(Node::list)
    {
        for tier in tiers.iter().filter_map(Node::list) {
            for value in tier.iter().filter_map(Node::bytes) {
                push_unique(
                    &mut all,
                    &mut seen,
                    String::from_utf8_lossy(value).into_owned(),
                );
            }
        }
    }
    if let Some(value) = entries.get(b"announce".as_slice()).and_then(Node::bytes) {
        push_unique(
            &mut all,
            &mut seen,
            String::from_utf8_lossy(value).into_owned(),
        );
    }
    let previous = all.len();
    for tracker in trackers {
        push_unique(&mut all, &mut seen, tracker.clone());
    }
    if previous == all.len() {
        return Ok(Cow::Borrowed(bytes));
    }
    let tiers = all
        .iter()
        .map(|s| Node::new(Value::List(vec![Node::new(Value::Bytes(s.as_bytes()))])))
        .collect();
    entries.insert(b"announce-list", Node::new(Value::List(tiers)));
    entries.insert(b"announce", Node::new(Value::Bytes(all[0].as_bytes())));
    // The root changed: re-encode it, keeping the source bytes of every untouched value.
    root.span = 0..0;
    Ok(Cow::Owned(bencode::encode_with_source(&root, bytes)))
}
