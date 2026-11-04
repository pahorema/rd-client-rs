//! The rules that decide which provider files of a torrent get downloaded.

use crate::{Provider, Torrent, TorrentDownloadAction};

/// Bytes per unit of [`Torrent::download_min_size`].
const BYTES_PER_MIB: i64 = 1024 * 1024;

/// The compiled file selection rules of one torrent.
///
/// Real-Debrid torrents with manually selected files always pass the size
/// rule; otherwise a [`Torrent::download_min_size`] of 0 or less disables
/// it, else the file must be strictly larger than that many MiB.
#[derive(Debug, Clone)]
pub struct FileFilter {
    /// Files must be strictly larger than this; `None` disables the rule.
    min_size: Option<i64>,
}

impl FileFilter {
    /// Compiles the rules of `torrent`.
    pub fn new(torrent: &Torrent) -> Self {
        let manual_real_debrid = torrent.client_kind == Some(Provider::RealDebrid)
            && torrent.download_action == TorrentDownloadAction::DownloadManual;
        let min_size = (!manual_real_debrid && torrent.download_min_size > 0)
            .then(|| i64::from(torrent.download_min_size) * BYTES_PER_MIB);
        Self { min_size }
    }

    /// Whether the provider file at `path` with `size` bytes gets downloaded.
    pub fn is_downloadable(&self, path: &str, size: i64) -> bool {
        if let Some(min_size) = self.min_size
            && size <= min_size
        {
            tracing::debug!(
                path,
                size,
                min_size,
                "file skipped: not larger than the minimum size"
            );
            return false;
        }
        tracing::debug!(path, "file included after filtering");
        true
    }
}

impl Torrent {
    /// Compiles this torrent's file selection rules once, for checking many
    /// files; see [`FileFilter`].
    pub fn file_filter(&self) -> FileFilter {
        FileFilter::new(self)
    }

    /// Whether the provider file at `path` with `size` bytes gets downloaded;
    /// see [`FileFilter`] for the rules.
    pub fn is_downloadable(&self, path: &str, size: i64) -> bool {
        self.file_filter().is_downloadable(path, size)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIB: i64 = 1024 * 1024;

    fn torrent() -> Torrent {
        Torrent {
            rd_id: Some("1".to_owned()),
            ..Torrent::default()
        }
    }

    fn with_min_size(download_min_size: i32) -> Torrent {
        Torrent {
            download_min_size,
            ..torrent()
        }
    }

    #[test]
    fn without_rules_every_file_is_downloadable() {
        assert!(torrent().is_downloadable("file.txt", 10_000));
    }

    #[test]
    fn files_not_larger_than_the_minimum_size_are_skipped() {
        for (min_size, size) in [(100, 20 * MIB), (2, 2 * MIB), (2, 2 * (1000 * 1000 + 1))] {
            assert!(
                !with_min_size(min_size).is_downloadable("file.txt", size),
                "min {min_size} MiB, size {size}"
            );
        }
    }

    #[test]
    fn files_larger_than_the_minimum_size_are_downloadable() {
        for (min_size, size) in [(100, 110 * MIB), (2, 2 * MIB + 1)] {
            assert!(
                with_min_size(min_size).is_downloadable("file.txt", size),
                "min {min_size} MiB, size {size}"
            );
        }
    }

    #[test]
    fn large_minimum_sizes_do_not_overflow() {
        let torrent = with_min_size(4096);
        assert!(!torrent.is_downloadable("file.txt", 4096 * MIB));
        assert!(torrent.is_downloadable("file.txt", 4096 * MIB + 1));
        let torrent = with_min_size(i32::MAX);
        assert!(!torrent.is_downloadable("file.txt", i64::from(i32::MAX) * MIB));
    }

    #[test]
    fn non_positive_minimum_size_disables_the_size_rule() {
        assert!(with_min_size(0).is_downloadable("file.txt", 0));
        assert!(with_min_size(-5).is_downloadable("file.txt", 0));
    }

    #[test]
    fn manual_real_debrid_selection_bypasses_the_size_rule() {
        let manual = Torrent {
            client_kind: Some(Provider::RealDebrid),
            download_action: TorrentDownloadAction::DownloadManual,
            ..with_min_size(10)
        };
        assert!(manual.is_downloadable("file.txt", 1));

        let other_provider = Torrent {
            client_kind: Some(Provider::AllDebrid),
            ..manual.clone()
        };
        assert!(!other_provider.is_downloadable("file.txt", 1));

        let not_manual = Torrent {
            download_action: TorrentDownloadAction::DownloadAll,
            ..manual
        };
        assert!(!not_manual.is_downloadable("file.txt", 1));
    }
}
