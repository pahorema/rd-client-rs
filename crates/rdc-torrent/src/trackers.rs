use std::collections::HashSet;
use url::Url;

/// Parses a newline tracker list, returning valid distinct URLs and rejected line count.
/// Empty lines and comments are ignored; duplicates are not counted as rejected.
/// URL spelling and order are preserved, with a trailing run of slashes removed before trimming.
#[must_use]
pub fn parse_tracker_list(text: &str) -> (Vec<String>, usize) {
    let mut accepted = Vec::new();
    let mut seen = HashSet::new();
    let mut rejected = 0;
    for line in text.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        let tracker = line.trim_end_matches('/').trim();
        if !valid_tracker(tracker) {
            rejected += 1;
            continue;
        }
        if seen.insert(tracker.to_lowercase()) {
            accepted.push(tracker.to_owned());
        }
    }
    (accepted, rejected)
}

fn valid_tracker(tracker: &str) -> bool {
    if tracker.contains("..") || tracker.contains('\\') || tracker.chars().any(char::is_control) {
        return false;
    }
    let Ok(url) = Url::parse(tracker) else {
        return false;
    };
    if !matches!(url.scheme(), "http" | "https" | "udp" | "wss") {
        return false;
    }
    let Some(host) = url.host_str().filter(|h| !h.is_empty()) else {
        return false;
    };
    let ipv6 = host.starts_with('[') && host.ends_with(']');
    host.chars().all(|c| {
        c.is_alphanumeric()
            || if ipv6 {
                ".:[]".contains(c)
            } else {
                ".-_".contains(c)
            }
    })
}
