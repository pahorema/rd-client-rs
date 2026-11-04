use crate::{MagnetLink, TorrentFile};

/// Returns the user-visible rejection for the first matching comma-separated banned entry.
/// Tracker comparisons trim whitespace and match case-insensitive substrings.
#[must_use]
pub fn banned_magnet_error(setting: &str, magnet: &MagnetLink) -> Option<String> {
    banned_error(setting, None, &magnet.trackers.iter().map(String::as_str))
}

/// Checks source markers before tracker URLs for each nonblank banned entry.
/// Source comparison deliberately uses the untrimmed, case-sensitive setting entry.
/// All announce-list tiers are checked, falling back to announce when tiers are absent.
#[must_use]
pub fn banned_torrent_error(setting: &str, torrent: &TorrentFile) -> Option<String> {
    let urls = torrent.announce_list.iter().flatten().map(String::as_str);
    if torrent.announce_list.is_empty() {
        banned_error(
            setting,
            torrent.source.as_deref(),
            &torrent.announce.iter().map(String::as_str),
        )
    } else {
        banned_error(setting, torrent.source.as_deref(), &urls)
    }
}

fn banned_error<'src>(
    setting: &str,
    source: Option<&str>,
    urls: &(impl Iterator<Item = &'src str> + Clone),
) -> Option<String> {
    for entry in setting.split(',') {
        let compare = entry.trim().to_lowercase();
        if compare.is_empty() {
            continue;
        }
        if let Some(source) = source.filter(|s| !s.trim().is_empty() && s.contains(entry)) {
            return Some(format!(
                "Cannot add torrent, the torrent source '{source}' is a banned tracker."
            ));
        }
        let matched: Vec<_> = urls
            .clone()
            .filter(|url| url.trim().to_lowercase().contains(&compare))
            .collect();
        if !matched.is_empty() {
            return Some(format!(
                "Cannot add torrent, the torrent contains banned trackers: {}.",
                matched.join(", ")
            ));
        }
    }
    None
}
