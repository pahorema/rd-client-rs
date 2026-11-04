//! The rules that decide which provider files of a torrent get downloaded.

use std::fmt;

use fancy_regex::Regex;

use crate::{Provider, Torrent, TorrentDownloadAction};

/// Bytes per unit of [`Torrent::download_min_size`].
const BYTES_PER_MIB: i64 = 1024 * 1024;

/// Which of the two torrent expressions an error refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    Include,
    Exclude,
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Include => "include",
            Self::Exclude => "exclude",
        })
    }
}

/// Error raised by a torrent's include or exclude regular expression.
#[derive(Debug)]
pub struct FilterError {
    role: Role,
    pattern: String,
    /// The path being matched; `None` when the expression does not compile.
    path: Option<String>,
    source: fancy_regex::Error,
}

impl FilterError {
    /// The offending regular expression.
    pub fn pattern(&self) -> &str {
        &self.pattern
    }
}

impl fmt::Display for FilterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self {
            role,
            pattern,
            path,
            source,
        } = self;
        match path {
            None => write!(f, "invalid {role} regular expression {pattern:?}: {source}"),
            Some(path) => write!(
                f,
                "{role} regular expression {pattern:?} could not be matched against {path:?}: {source}"
            ),
        }
    }
}

impl std::error::Error for FilterError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

/// The compiled file selection rules of one torrent.
///
/// A file is downloadable when it passes the size rule and the path rule:
/// - size: Real-Debrid torrents with manually selected files always pass;
///   otherwise a [`Torrent::download_min_size`] of 0 or less disables the
///   rule, else the file must be strictly larger than that many MiB;
/// - path: a non-blank include expression must match the path, and the
///   exclude expression is then ignored; otherwise a non-blank exclude
///   expression must not match. Expressions search anywhere in the full
///   provider path (e.g. `folder/sub/file.ext`) and support look-around and
///   back-references.
#[derive(Debug, Clone)]
pub struct FileFilter {
    /// Files must be strictly larger than this; `None` disables the rule.
    min_size: Option<i64>,
    include: Option<Regex>,
    exclude: Option<Regex>,
}

impl FileFilter {
    /// Compiles the rules of `torrent`.
    ///
    /// # Errors
    ///
    /// Returns an error when the expression in effect does not compile (an
    /// exclude expression shadowed by an include expression is not checked).
    pub fn new(torrent: &Torrent) -> Result<Self, FilterError> {
        let manual_real_debrid = torrent.client_kind == Some(Provider::RealDebrid)
            && torrent.download_action == TorrentDownloadAction::DownloadManual;
        let min_size = (!manual_real_debrid && torrent.download_min_size > 0)
            .then(|| i64::from(torrent.download_min_size) * BYTES_PER_MIB);
        let include = compile(Role::Include, torrent.include_regex.as_deref())?;
        let exclude = if include.is_some() {
            None
        } else {
            compile(Role::Exclude, torrent.exclude_regex.as_deref())?
        };
        Ok(Self {
            min_size,
            include,
            exclude,
        })
    }

    /// Whether the provider file at `path` with `size` bytes gets downloaded.
    ///
    /// # Errors
    ///
    /// Returns an error when matching an expression exceeds the regex
    /// engine's backtracking limit.
    pub fn is_downloadable(&self, path: &str, size: i64) -> Result<bool, FilterError> {
        if let Some(min_size) = self.min_size
            && size <= min_size
        {
            tracing::debug!(
                path,
                size,
                min_size,
                "file skipped: not larger than the minimum size"
            );
            return Ok(false);
        }
        if let Some(include) = &self.include
            && !matches(Role::Include, include, path)?
        {
            tracing::debug!(
                path,
                include = include.as_str(),
                "file skipped: include expression does not match"
            );
            return Ok(false);
        }
        if let Some(exclude) = &self.exclude
            && matches(Role::Exclude, exclude, path)?
        {
            tracing::debug!(
                path,
                exclude = exclude.as_str(),
                "file skipped: exclude expression matches"
            );
            return Ok(false);
        }
        tracing::debug!(path, "file included after filtering");
        Ok(true)
    }
}

fn compile(role: Role, pattern: Option<&str>) -> Result<Option<Regex>, FilterError> {
    let Some(pattern) = pattern.filter(|pattern| !pattern.trim().is_empty()) else {
        return Ok(None);
    };
    Regex::new(pattern).map(Some).map_err(|source| FilterError {
        role,
        pattern: pattern.to_owned(),
        path: None,
        source,
    })
}

fn matches(role: Role, regex: &Regex, path: &str) -> Result<bool, FilterError> {
    regex.is_match(path).map_err(|source| FilterError {
        role,
        pattern: regex.as_str().to_owned(),
        path: Some(path.to_owned()),
        source,
    })
}

impl Torrent {
    /// Compiles this torrent's file selection rules once, for checking many
    /// files; see [`FileFilter`].
    ///
    /// # Errors
    ///
    /// Returns an error when the expression in effect does not compile.
    pub fn file_filter(&self) -> Result<FileFilter, FilterError> {
        FileFilter::new(self)
    }

    /// Whether the provider file at `path` with `size` bytes gets downloaded;
    /// see [`FileFilter`] for the rules.
    ///
    /// # Errors
    ///
    /// Returns an error when an expression does not compile or cannot be
    /// matched.
    pub fn is_downloadable(&self, path: &str, size: i64) -> Result<bool, FilterError> {
        self.file_filter()?.is_downloadable(path, size)
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

    fn with_include(include: &str) -> Torrent {
        Torrent {
            include_regex: Some(include.to_owned()),
            ..torrent()
        }
    }

    fn with_exclude(exclude: &str) -> Torrent {
        Torrent {
            exclude_regex: Some(exclude.to_owned()),
            ..torrent()
        }
    }

    const NON_MATCHING: [(&str, &str); 4] = [
        ("file", "no-match"),
        ("file", "even/in/a/subdirectory.txt"),
        ("ch[aA]racter c[lL]asses", "nope.txt"),
        ("digits\\d+", "123 not matching.txt"),
    ];

    const MATCHING: [(&str, &str); 4] = [
        ("file", "file.txt"),
        ("file", "file/in/a/subdirectory.txt"),
        ("ch[aA]racter c[lL]asses", "character cLasses"),
        ("digits\\d+", "digits123456.txt"),
    ];

    #[test]
    fn without_rules_every_file_is_downloadable() {
        assert!(torrent().is_downloadable("file.txt", 10_000).unwrap());
    }

    #[test]
    fn files_not_larger_than_the_minimum_size_are_skipped() {
        for (min_size, size) in [(100, 20 * MIB), (2, 2 * MIB), (2, 2 * (1000 * 1000 + 1))] {
            assert!(
                !with_min_size(min_size)
                    .is_downloadable("file.txt", size)
                    .unwrap(),
                "min {min_size} MiB, size {size}"
            );
        }
    }

    #[test]
    fn files_larger_than_the_minimum_size_are_downloadable() {
        for (min_size, size) in [(100, 110 * MIB), (2, 2 * MIB + 1)] {
            assert!(
                with_min_size(min_size)
                    .is_downloadable("file.txt", size)
                    .unwrap(),
                "min {min_size} MiB, size {size}"
            );
        }
    }

    #[test]
    fn large_minimum_sizes_do_not_overflow() {
        let torrent = with_min_size(4096);
        assert!(!torrent.is_downloadable("file.txt", 4096 * MIB).unwrap());
        assert!(torrent.is_downloadable("file.txt", 4096 * MIB + 1).unwrap());
        let torrent = with_min_size(i32::MAX);
        assert!(
            !torrent
                .is_downloadable("file.txt", i64::from(i32::MAX) * MIB)
                .unwrap()
        );
    }

    #[test]
    fn non_positive_minimum_size_disables_the_size_rule() {
        assert!(with_min_size(0).is_downloadable("file.txt", 0).unwrap());
        assert!(with_min_size(-5).is_downloadable("file.txt", 0).unwrap());
    }

    #[test]
    fn manual_real_debrid_selection_bypasses_the_size_rule() {
        let manual = Torrent {
            client_kind: Some(Provider::RealDebrid),
            download_action: TorrentDownloadAction::DownloadManual,
            ..with_min_size(10)
        };
        assert!(manual.is_downloadable("file.txt", 1).unwrap());

        let other_provider = Torrent {
            client_kind: Some(Provider::AllDebrid),
            ..manual.clone()
        };
        assert!(!other_provider.is_downloadable("file.txt", 1).unwrap());

        let not_manual = Torrent {
            download_action: TorrentDownloadAction::DownloadAll,
            ..manual
        };
        assert!(!not_manual.is_downloadable("file.txt", 1).unwrap());
    }

    #[test]
    fn paths_not_matching_the_include_expression_are_skipped() {
        for (include, path) in NON_MATCHING {
            assert!(
                !with_include(include)
                    .is_downloadable(path, i64::MAX)
                    .unwrap(),
                "include {include:?}, path {path:?}"
            );
        }
    }

    #[test]
    fn paths_matching_the_include_expression_are_downloadable() {
        for (include, path) in MATCHING {
            assert!(
                with_include(include)
                    .is_downloadable(path, i64::MAX)
                    .unwrap(),
                "include {include:?}, path {path:?}"
            );
        }
    }

    #[test]
    fn paths_not_matching_the_exclude_expression_are_downloadable() {
        for (exclude, path) in NON_MATCHING {
            assert!(
                with_exclude(exclude)
                    .is_downloadable(path, i64::MAX)
                    .unwrap(),
                "exclude {exclude:?}, path {path:?}"
            );
        }
    }

    #[test]
    fn paths_matching_the_exclude_expression_are_skipped() {
        for (exclude, path) in MATCHING {
            assert!(
                !with_exclude(exclude)
                    .is_downloadable(path, i64::MAX)
                    .unwrap(),
                "exclude {exclude:?}, path {path:?}"
            );
        }
    }

    #[test]
    fn include_expression_wins_over_exclude_expression() {
        let cases = [
            ("file", "file", "file.txt"),
            ("file", "in/a", "file/in/a/subdirectory.txt"),
            ("ch[aA]racter c[lL]asses", "character", "character cLasses"),
            ("digits\\d+", "123456", "digits123456.txt"),
        ];
        for (include, exclude, path) in cases {
            let torrent = Torrent {
                exclude_regex: Some(exclude.to_owned()),
                ..with_include(include)
            };
            assert!(
                torrent.is_downloadable(path, i64::MAX).unwrap(),
                "include {include:?}, exclude {exclude:?}, path {path:?}"
            );
        }
    }

    #[test]
    fn large_file_not_matching_the_include_expression_is_skipped() {
        let torrent = Torrent {
            download_min_size: 10,
            ..with_include("file")
        };
        assert!(
            !torrent
                .is_downloadable("no-match.txt", 10 * MIB + 1)
                .unwrap()
        );
    }

    #[test]
    fn small_file_matching_the_include_expression_is_skipped() {
        let torrent = Torrent {
            download_min_size: 10,
            ..with_include("file")
        };
        assert!(!torrent.is_downloadable("file.txt", 10 * MIB - 1).unwrap());
    }

    #[test]
    fn blank_expressions_are_ignored() {
        let torrent = Torrent {
            include_regex: Some("  ".to_owned()),
            exclude_regex: Some(String::new()),
            ..torrent()
        };
        assert!(torrent.is_downloadable("anything", 1).unwrap());
    }

    #[test]
    fn look_around_and_back_references_are_supported() {
        let torrent = with_include(r"^(?!.*sample).*\.mkv$");
        assert!(torrent.is_downloadable("show/episode.mkv", 1).unwrap());
        assert!(!torrent.is_downloadable("show/sample.mkv", 1).unwrap());
        let torrent = with_exclude(r"(\w)\1");
        assert!(!torrent.is_downloadable("aab", 1).unwrap());
        assert!(torrent.is_downloadable("abc", 1).unwrap());
    }

    #[test]
    fn invalid_expressions_are_errors() {
        let error = with_include("(unclosed")
            .is_downloadable("file", 1)
            .unwrap_err();
        assert_eq!(error.pattern(), "(unclosed");
        assert!(
            error
                .to_string()
                .starts_with("invalid include regular expression")
        );
        let error = with_exclude("[").file_filter().unwrap_err();
        assert!(
            error
                .to_string()
                .starts_with("invalid exclude regular expression")
        );
    }

    #[test]
    fn shadowed_exclude_expression_is_not_compiled() {
        let torrent = Torrent {
            exclude_regex: Some("[".to_owned()),
            ..with_include("file")
        };
        assert!(torrent.is_downloadable("file.txt", 1).unwrap());
    }
}
