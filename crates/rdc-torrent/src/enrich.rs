use std::{borrow::Cow, collections::HashSet};

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
