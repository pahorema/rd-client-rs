//! Where downloads land on disk and how provider names become safe file
//! and directory names.

use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use url::Url;

use crate::{Download, DownloadInfo, Torrent};

/// Whether the platform forbids `c` in a path (Unix: NUL; Windows: NUL,
/// the other control characters and `|`).
fn is_invalid_path_char(c: char) -> bool {
    if cfg!(windows) {
        c == '|' || c < ' '
    } else {
        c == '\0'
    }
}

/// Whether the platform forbids `c` in a file name (Unix: NUL and `/`;
/// Windows: the invalid path characters plus `" < > : * ? \ /`).
fn is_invalid_file_name_char(c: char) -> bool {
    if cfg!(windows) {
        is_invalid_path_char(c) || matches!(c, '"' | '<' | '>' | ':' | '*' | '?' | '\\' | '/')
    } else {
        c == '\0' || c == '/'
    }
}

/// Turns a provider file name into a single safe path component.
///
/// Removes the characters the platform forbids in file names and both `/`
/// and `\` on every platform, then every `..` sequence (left to right,
/// without overlap): `test/../file.txt` becomes `testfile.txt`,
/// `.../test.txt` becomes `.test.txt` and `test....txt` becomes `testtxt`.
pub fn remove_invalid_file_name_chars(name: &str) -> String {
    let kept: String = name
        .chars()
        .filter(|&c| !is_invalid_file_name_char(c) && c != '/' && c != '\\')
        .collect();
    kept.replace("..", "")
}

/// Removes the characters the platform forbids anywhere in a path (Unix:
/// NUL; Windows: NUL, the other control characters and `|`).
pub fn remove_invalid_path_chars(path: &str) -> String {
    path.chars().filter(|&c| !is_invalid_path_char(c)).collect()
}

/// The local file name of a download, sanitized with
/// [`remove_invalid_file_name_chars`].
///
/// Uses [`Download::file_name`] when it is not blank, otherwise the
/// URL-decoded last path segment of the unrestricted [`Download::link`].
/// `None` when the link is blank, or when the name has to come from a link
/// that is not an absolute URL. The result may be empty (a link without a
/// path, e.g. `https://host`).
pub fn file_name(download: &Download) -> Option<String> {
    let link = non_blank(download.link.as_deref())?;
    let name = raw_file_name(download, link)?;
    Some(remove_invalid_file_name_chars(&name))
}

/// Absolute path the download is written to, creating its directory.
///
/// The layout is `base/<torrent name>/<provider sub-directories>/<file>`:
/// the torrent name is [`Torrent::rd_name`] without the characters the
/// platform forbids in paths; the sub-directories are those of the first
/// provider file whose path ends with the [`file_name`], without a leading
/// directory equal to the torrent name (compared case-insensitively). A
/// blank file name matches no provider file and yields the torrent
/// directory itself. Only plain components are used, so `..`, `.` and
/// absolute names never leave `base`.
///
/// Returns `Ok(None)` when the torrent has no name, the download has no
/// unrestricted link or no file name can be derived.
///
/// # Errors
///
/// Returns the I/O error raised while creating the directory.
pub fn download_path(
    base: &Path,
    torrent: &Torrent,
    download: &Download,
) -> io::Result<Option<PathBuf>> {
    let (Some(rd_name), Some(_)) = (
        torrent.rd_name.as_deref(),
        non_blank(download.link.as_deref()),
    ) else {
        return Ok(None);
    };
    let Some(file_name) = file_name(download) else {
        return Ok(None);
    };
    let directory = remove_invalid_path_chars(rd_name);
    let mut path = base.to_path_buf();
    push_components(&mut path, &directory);
    if let Some(sub_directory) = sub_directory(torrent, &file_name, &directory) {
        push_components(&mut path, &sub_directory);
    }
    fs::create_dir_all(&path)?;
    if !file_name.is_empty() {
        path.push(file_name);
    }
    Ok(Some(path))
}

/// Path of the download relative to the download directory, the layout of
/// [`download_path`] without the base directory.
///
/// Unlike [`download_path`] the file name is not sanitized (the raw
/// [`Download::file_name`] or URL-decoded link segment is used) and no
/// directory is created. This is the path a download has below the root of
/// a remote downloader or an rclone mount.
///
/// Returns `None` when the torrent has no name, the download has no
/// unrestricted link or no file name can be derived.
pub fn relative_download_path(torrent: &Torrent, download: &Download) -> Option<String> {
    let rd_name = torrent.rd_name.as_deref()?;
    let link = non_blank(download.link.as_deref())?;
    let file_name = raw_file_name(download, link)?;
    let directory = remove_invalid_path_chars(rd_name);
    let mut path = PathBuf::new();
    push_components(&mut path, &directory);
    if let Some(sub_directory) = sub_directory(torrent, &file_name, &directory) {
        push_components(&mut path, &sub_directory);
    }
    push_components(&mut path, &file_name);
    Some(path.to_string_lossy().into_owned())
}

fn non_blank(text: Option<&str>) -> Option<&str> {
    text.filter(|text| !text.trim().is_empty())
}

/// The unsanitized file name: the download's own name or the link's.
fn raw_file_name(download: &Download, link: &str) -> Option<String> {
    match non_blank(download.file_name.as_deref()) {
        Some(name) => Some(name.to_owned()),
        None => link_file_name(link),
    }
}

/// The URL-decoded last path segment of an absolute URL; a trailing `/`
/// belongs to the segment (`https://host/dir/` gives `dir/`, a bare host
/// gives `/`).
fn link_file_name(link: &str) -> Option<String> {
    let url = Url::parse(link).ok()?;
    Some(url_decode(last_segment(url.path())))
}

fn last_segment(path: &str) -> &str {
    let without_trailing = path.strip_suffix('/').unwrap_or(path);
    without_trailing
        .rfind('/')
        .map_or(path, |slash| &path[slash + 1..])
}

/// Decodes `%XX` escapes (as UTF-8, invalid sequences replaced) and `+` as
/// a space; malformed escapes are kept literally.
fn url_decode(text: &str) -> String {
    fn hex(byte: u8) -> Option<u8> {
        match byte {
            b'0'..=b'9' => Some(byte - b'0'),
            b'a'..=b'f' => Some(byte - b'a' + 10),
            b'A'..=b'F' => Some(byte - b'A' + 10),
            _ => None,
        }
    }

    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while let Some(&byte) = bytes.get(index) {
        let escaped = (byte == b'%')
            .then(|| Some((hex(*bytes.get(index + 1)?)?, hex(*bytes.get(index + 2)?)?)))
            .flatten();
        match (byte, escaped) {
            (_, Some((high, low))) => {
                decoded.push((high << 4) | low);
                index += 3;
            }
            (b'+', None) => {
                decoded.push(b' ');
                index += 1;
            }
            (_, None) => {
                decoded.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

/// Appends the plain components of `relative`, dropping roots, prefixes,
/// `.` and `..`.
fn push_components(path: &mut PathBuf, relative: &str) {
    for component in Path::new(relative).components() {
        if let Component::Normal(part) = component {
            path.push(part);
        }
    }
}

/// The provider sub-directories of the first provider file whose path ends
/// with `file_name`, without a leading directory equal to the torrent
/// directory. A blank `file_name` matches nothing.
fn sub_directory(torrent: &Torrent, file_name: &str, directory: &str) -> Option<String> {
    if file_name.trim().is_empty() {
        return None;
    }
    let files = torrent.files();
    let file = files
        .iter()
        .find(|file| !file.path.trim().is_empty() && file.path.ends_with(file_name))?;
    let parent = Path::new(&file.path).parent()?.to_str()?;
    let trimmed = parent.trim_matches('/').trim_matches('\\');
    let stripped = strip_torrent_name_prefix(trimmed, directory);
    (!stripped.trim().is_empty()).then(|| stripped.to_owned())
}

/// Drops the first component of `sub_path` (split on `/` or `\`) when it
/// names the torrent directory.
fn strip_torrent_name_prefix<'a>(sub_path: &'a str, directory: &str) -> &'a str {
    let (first, rest) = sub_path.split_once(['/', '\\']).unwrap_or((sub_path, ""));
    let directory = directory.trim_end_matches(['/', '\\']);
    if eq_ignore_case(&remove_invalid_path_chars(first), directory) {
        rest
    } else {
        sub_path
    }
}

/// Size in bytes the provider reports for the download's file.
///
/// `None` unless exactly one provider file with a non-blank path ends with
/// the download's [`file_name`] and reports a positive size; a blank file
/// name matches nothing.
pub fn expected_file_size(torrent: &Torrent, download: &Download) -> Option<i64> {
    let file_name = file_name(download).filter(|name| !name.trim().is_empty())?;
    let files = torrent.files();
    let mut matching = files
        .iter()
        .filter(|file| !file.path.trim().is_empty() && file.path.ends_with(&file_name));
    let file = matching.next()?;
    if matching.next().is_some() {
        return None;
    }
    (file.bytes > 0).then_some(file.bytes)
}

/// Finds the refreshed provider link that belongs to `download` among the
/// links the provider currently reports for its torrent.
///
/// File names are not unique inside a torrent, so a name only counts when
/// it identifies exactly one link; an ambiguous name yields `None` rather
/// than a sibling's link. In order:
/// 1. a link equal to [`Download::path`] (the stored restricted link);
/// 2. when [`Download::file_name`] is not blank, the infos with that file
///    name (case-insensitive), if exactly one, else `None`;
/// 3. the infos whose file name or own link's last segment equals the last
///    segment of [`Download::path`] (URL-decoded for URLs; case-insensitive),
///    if exactly one, else `None`;
/// 4. the only info, when there is exactly one.
///
/// Blank names never match.
pub fn match_download_info<'a>(
    infos: &'a [DownloadInfo],
    download: &Download,
) -> Option<&'a DownloadInfo> {
    if let Some(same_link) = infos
        .iter()
        .find(|info| info.restricted_link == download.path)
    {
        return Some(same_link);
    }
    if let Some(file_name) = non_blank(download.file_name.as_deref()) {
        let mut by_file_name = infos
            .iter()
            .filter(|info| info_file_name_is(info, file_name))
            .peekable();
        if by_file_name.peek().is_some() {
            return single(by_file_name);
        }
    }
    if let Some(path_file_name) =
        path_file_name(&download.path).filter(|name| !name.trim().is_empty())
    {
        let mut by_path_file_name = infos
            .iter()
            .filter(|info| {
                info_file_name_is(info, &path_file_name)
                    || path_file_name_is(&info.restricted_link, &path_file_name)
            })
            .peekable();
        if by_path_file_name.peek().is_some() {
            return single(by_path_file_name);
        }
    }
    match infos {
        [only] => Some(only),
        _ => None,
    }
}

/// The only item of `candidates`, or `None` when there are several.
fn single<T>(mut candidates: impl Iterator<Item = T>) -> Option<T> {
    let first = candidates.next()?;
    candidates.next().is_none().then_some(first)
}

fn info_file_name_is(info: &DownloadInfo, name: &str) -> bool {
    non_blank(info.file_name.as_deref()).is_some_and(|info_name| eq_ignore_case(info_name, name))
}

fn path_file_name_is(path: &str, name: &str) -> bool {
    path_file_name(path).is_some_and(|path_name| eq_ignore_case(&path_name, name))
}

/// The last segment of a link (URL-decoded) or of a plain path; `None` for
/// a blank input.
fn path_file_name(path: &str) -> Option<String> {
    if path.trim().is_empty() {
        return None;
    }
    // A one-letter scheme is a Windows drive, not a URL.
    if let Ok(url) = Url::parse(path)
        && url.scheme().len() > 1
    {
        return Some(url_decode(last_segment(url.path())));
    }
    let name = path.rsplit(std::path::is_separator).next().unwrap_or(path);
    Some(name.to_owned())
}

/// Case-insensitive comparison using simple Unicode upper-casing.
pub(crate) fn eq_ignore_case(left: &str, right: &str) -> bool {
    left.chars()
        .flat_map(char::to_uppercase)
        .eq(right.chars().flat_map(char::to_uppercase))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every character the platform forbids in a file name.
    fn invalid_file_name_chars() -> String {
        (0..=0x7f_u8)
            .map(char::from)
            .filter(|&c| is_invalid_file_name_char(c))
            .collect()
    }

    #[test]
    fn file_name_loses_invalid_chars() {
        let input = format!("test{}file.txt", invalid_file_name_chars());
        assert_eq!(remove_invalid_file_name_chars(&input), "testfile.txt");
    }

    #[test]
    fn platform_file_name_char_set() {
        if cfg!(windows) {
            let mut expected: String = (1..32_u8).map(char::from).collect();
            expected.insert(0, '\0');
            expected.push_str("\"*/:<>?\\|");
            let mut actual: Vec<char> = invalid_file_name_chars().chars().collect();
            actual.sort_unstable();
            let mut expected: Vec<char> = expected.chars().collect();
            expected.sort_unstable();
            assert_eq!(actual, expected);
        } else {
            assert_eq!(invalid_file_name_chars(), "\0/");
        }
    }

    #[test]
    fn file_name_loses_directory_separators() {
        assert_eq!(
            remove_invalid_file_name_chars("folder/subfolder\\file.txt"),
            "foldersubfolderfile.txt"
        );
    }

    #[test]
    fn file_name_loses_double_dots() {
        assert_eq!(
            remove_invalid_file_name_chars("test..file.txt"),
            "testfile.txt"
        );
    }

    #[test]
    fn file_name_loses_leading_separators() {
        assert_eq!(
            remove_invalid_file_name_chars("/test/file.txt"),
            "testfile.txt"
        );
    }

    #[test]
    fn valid_file_name_is_unchanged() {
        assert_eq!(
            remove_invalid_file_name_chars("valid_file-123.txt"),
            "valid_file-123.txt"
        );
    }

    #[test]
    fn empty_file_name_stays_empty() {
        assert_eq!(remove_invalid_file_name_chars(""), "");
    }

    #[test]
    fn file_name_complex_cases() {
        let cases = [
            ("test/../file.txt", "testfile.txt"),
            (".../test.txt", ".test.txt"),
            ("test....txt", "testtxt"),
        ];
        for (input, expected) in cases {
            assert_eq!(
                remove_invalid_file_name_chars(input),
                expected,
                "input {input:?}"
            );
        }
    }

    #[test]
    fn path_loses_platform_invalid_chars() {
        assert_eq!(remove_invalid_path_chars("Some\0 Name"), "Some Name");
        if cfg!(windows) {
            assert_eq!(remove_invalid_path_chars("a|b\u{1f}c/d"), "abc/d");
        } else {
            assert_eq!(remove_invalid_path_chars("a|b\u{1f}c/d"), "a|b\u{1f}c/d");
        }
    }

    mod download_paths {
        use std::path::MAIN_SEPARATOR_STR;

        use super::super::*;
        use crate::RemoteFile;

        const TORRENT_NAME: &str = "Torrent Name";

        fn download(link: Option<&str>, file_name: Option<&str>) -> Download {
            Download {
                link: link.map(str::to_owned),
                file_name: file_name.map(str::to_owned),
                ..Download::default()
            }
        }

        fn torrent(rd_name: Option<&str>, file_paths: &[&[&str]]) -> Torrent {
            let mut torrent = Torrent {
                rd_name: rd_name.map(str::to_owned),
                ..Torrent::default()
            };
            if !file_paths.is_empty() {
                let files: Vec<RemoteFile> = file_paths
                    .iter()
                    .map(|parts| RemoteFile {
                        path: parts.join(MAIN_SEPARATOR_STR),
                        ..RemoteFile::default()
                    })
                    .collect();
                torrent.set_files(&files);
            }
            torrent
        }

        fn joined(parts: &[&str]) -> String {
            parts
                .iter()
                .collect::<PathBuf>()
                .to_string_lossy()
                .into_owned()
        }

        fn file_download() -> Download {
            download(Some("https://fake.url/file.txt"), Some("file.txt"))
        }

        #[test]
        fn with_base_returns_none_without_torrent_name() {
            let base = tempfile::tempdir().unwrap();
            let path = download_path(base.path(), &torrent(None, &[]), &file_download()).unwrap();
            assert_eq!(path, None);
        }

        #[test]
        fn relative_returns_none_without_torrent_name() {
            assert_eq!(
                relative_download_path(&torrent(None, &[]), &file_download()),
                None
            );
        }

        #[test]
        fn with_base_returns_none_without_link() {
            let base = tempfile::tempdir().unwrap();
            let download = download(None, Some("file.txt"));
            let torrent = torrent(Some(TORRENT_NAME), &[]);
            assert_eq!(
                download_path(base.path(), &torrent, &download).unwrap(),
                None
            );
            assert!(!base.path().join(TORRENT_NAME).exists());
        }

        #[test]
        fn relative_returns_none_without_link() {
            let download = download(None, Some("file.txt"));
            assert_eq!(
                relative_download_path(&torrent(Some(TORRENT_NAME), &[]), &download),
                None
            );
        }

        #[test]
        fn with_base_guesses_the_file_name_from_the_link() {
            let base = tempfile::tempdir().unwrap();
            let download = download(Some("https://fake.url/filename-from-link.txt"), None);
            let path = download_path(base.path(), &torrent(Some(TORRENT_NAME), &[]), &download);
            assert_eq!(
                path.unwrap(),
                Some(
                    base.path()
                        .join(TORRENT_NAME)
                        .join("filename-from-link.txt")
                )
            );
        }

        #[test]
        fn relative_guesses_the_file_name_from_the_link() {
            let download = download(Some("https://fake.url/filename-from-link.txt"), None);
            assert_eq!(
                relative_download_path(&torrent(Some(TORRENT_NAME), &[]), &download),
                Some(joined(&[TORRENT_NAME, "filename-from-link.txt"]))
            );
        }

        #[test]
        fn with_base_creates_the_torrent_directory() {
            let base = tempfile::tempdir().unwrap();
            let path = download_path(
                base.path(),
                &torrent(Some(TORRENT_NAME), &[]),
                &file_download(),
            );
            let directory = base.path().join(TORRENT_NAME);
            assert!(directory.is_dir());
            assert_eq!(path.unwrap(), Some(directory.join("file.txt")));
        }

        #[test]
        fn with_base_keeps_provider_sub_directories() {
            let base = tempfile::tempdir().unwrap();
            let file = ["inside", "lots", "of", "subdirectories", "file.txt"];
            let torrent = torrent(Some(TORRENT_NAME), &[&file]);
            let path = download_path(base.path(), &torrent, &file_download()).unwrap();
            let expected: PathBuf = [base.path(), Path::new(TORRENT_NAME)]
                .into_iter()
                .chain(file.iter().map(Path::new))
                .collect();
            assert_eq!(path, Some(expected.clone()));
            assert!(expected.parent().unwrap().is_dir());
        }

        #[test]
        fn relative_keeps_provider_sub_directories() {
            let file = ["inside", "lots", "of", "subdirectories", "file.txt"];
            let torrent = torrent(Some(TORRENT_NAME), &[&file]);
            assert_eq!(
                relative_download_path(&torrent, &file_download()),
                Some(joined(&[
                    TORRENT_NAME,
                    "inside",
                    "lots",
                    "of",
                    "subdirectories",
                    "file.txt"
                ]))
            );
        }

        #[test]
        fn with_base_strips_the_torrent_name_prefix() {
            let base = tempfile::tempdir().unwrap();
            let torrent = torrent(
                Some(TORRENT_NAME),
                &[&[TORRENT_NAME, "Saison 1", "file.txt"]],
            );
            let path = download_path(base.path(), &torrent, &file_download()).unwrap();
            assert_eq!(
                path,
                Some(
                    base.path()
                        .join(TORRENT_NAME)
                        .join("Saison 1")
                        .join("file.txt")
                )
            );
        }

        #[test]
        fn relative_strips_the_torrent_name_prefix() {
            let torrent = torrent(
                Some(TORRENT_NAME),
                &[&[TORRENT_NAME, "Saison 1", "file.txt"]],
            );
            assert_eq!(
                relative_download_path(&torrent, &file_download()),
                Some(joined(&[TORRENT_NAME, "Saison 1", "file.txt"]))
            );
        }

        #[test]
        fn with_base_returns_the_torrent_directory_when_no_file_name_is_known() {
            let base = tempfile::tempdir().unwrap();
            let torrent = torrent(Some(TORRENT_NAME), &[&["Other", "file.txt"]]);
            let download = download(Some("https://fake.url"), None);
            let path = download_path(base.path(), &torrent, &download).unwrap();
            assert_eq!(path, Some(base.path().join(TORRENT_NAME)));
        }

        #[test]
        fn torrent_name_prefix_is_compared_case_insensitively() {
            let torrent = torrent(Some("Torrent Ä"), &[&["torrent ä", "Saison 1", "file.txt"]]);
            assert_eq!(
                relative_download_path(&torrent, &file_download()),
                Some(joined(&["Torrent Ä", "Saison 1", "file.txt"]))
            );
        }

        #[test]
        fn provider_paths_with_a_leading_slash_keep_their_directories() {
            let torrent = torrent(Some(TORRENT_NAME), &[&["", "Season 1", "file.txt"]]);
            assert_eq!(
                relative_download_path(&torrent, &file_download()),
                Some(joined(&[TORRENT_NAME, "Season 1", "file.txt"]))
            );
        }

        #[test]
        fn names_cannot_escape_the_base_directory() {
            let base = tempfile::tempdir().unwrap();
            let torrent = torrent(Some(".."), &[&["..", "..", "file.txt"]]);
            let path = download_path(base.path(), &torrent, &file_download()).unwrap();
            assert_eq!(path, Some(base.path().join("file.txt")));
            let download = download(Some("https://fake.url/x"), Some("../../etc/passwd"));
            assert_eq!(
                relative_download_path(&torrent, &download),
                Some(joined(&["etc", "passwd"]))
            );
        }

        #[test]
        fn file_name_prefers_the_download_name_and_sanitizes_it() {
            let download = download(Some("https://fake.url/link.txt"), Some("sub/name.txt"));
            assert_eq!(file_name(&download).as_deref(), Some("subname.txt"));
        }

        #[test]
        fn file_name_decodes_the_link_segment() {
            let download = download(
                Some("https://fake.url/a/Fianc%C3%A9+S01.mkv?x=1"),
                Some(" "),
            );
            assert_eq!(file_name(&download).as_deref(), Some("Fiancé S01.mkv"));
        }

        #[test]
        fn file_name_needs_a_link() {
            assert_eq!(file_name(&download(None, Some("name.txt"))), None);
            assert_eq!(file_name(&download(Some("  "), Some("name.txt"))), None);
            assert_eq!(file_name(&download(Some("not a url"), None)), None);
        }

        #[test]
        fn url_decoding_keeps_malformed_escapes() {
            assert_eq!(url_decode("100%25%zz%4"), "100%%zz%4");
            assert_eq!(url_decode("%ff"), "\u{fffd}");
        }

        #[test]
        fn last_segment_keeps_a_trailing_slash() {
            assert_eq!(last_segment("/a/b.txt"), "b.txt");
            assert_eq!(last_segment("/a/dir/"), "dir/");
            assert_eq!(last_segment("/"), "/");
            assert_eq!(last_segment(""), "");
        }

        fn sized_torrent(files: &[(&str, i64)]) -> Torrent {
            let files: Vec<RemoteFile> = files
                .iter()
                .map(|&(path, bytes)| RemoteFile {
                    path: path.to_owned(),
                    bytes,
                    ..RemoteFile::default()
                })
                .collect();
            let mut torrent = torrent(Some(TORRENT_NAME), &[]);
            torrent.set_files(&files);
            torrent
        }

        #[test]
        fn expected_size_comes_from_the_single_matching_file() {
            let torrent = sized_torrent(&[("dir/file.txt", 42), ("dir/other.txt", 7)]);
            assert_eq!(expected_file_size(&torrent, &file_download()), Some(42));
        }

        #[test]
        fn expected_size_is_unknown_when_ambiguous_or_missing() {
            let torrent = sized_torrent(&[("a/file.txt", 42), ("b/file.txt", 42)]);
            assert_eq!(expected_file_size(&torrent, &file_download()), None);
            let torrent = sized_torrent(&[("dir/other.txt", 7)]);
            assert_eq!(expected_file_size(&torrent, &file_download()), None);
        }

        #[test]
        fn expected_size_ignores_blank_paths_and_non_positive_sizes() {
            let torrent = sized_torrent(&[(" ", 5), ("dir/file.txt", 0)]);
            assert_eq!(expected_file_size(&torrent, &file_download()), None);
        }

        #[test]
        fn expected_size_needs_a_file_name() {
            let torrent = sized_torrent(&[("dir/file.txt", 42)]);
            let download = download(Some("https://fake.url"), None);
            assert_eq!(expected_file_size(&torrent, &download), None);
        }
    }

    mod link_matcher {
        use super::super::*;

        fn info(file_name: Option<&str>, restricted_link: &str) -> DownloadInfo {
            DownloadInfo {
                file_name: file_name.map(str::to_owned),
                restricted_link: restricted_link.to_owned(),
            }
        }

        fn download(file_name: Option<&str>, path: &str) -> Download {
            Download {
                file_name: file_name.map(str::to_owned),
                path: path.to_owned(),
                ..Download::default()
            }
        }

        fn matched<'a>(infos: &'a [DownloadInfo], download: &Download) -> Option<&'a str> {
            match_download_info(infos, download).map(|info| info.restricted_link.as_str())
        }

        #[test]
        fn matches_by_file_name() {
            let download = download(
                Some("episode.mkv"),
                "https://cdn.example.com/old-token/episode.mkv",
            );
            let infos = [
                info(Some("other.mkv"), "https://cdn.example.com/new/other.mkv"),
                info(
                    Some("episode.mkv"),
                    "https://cdn.example.com/new/episode.mkv",
                ),
            ];
            assert_eq!(
                matched(&infos, &download),
                Some("https://cdn.example.com/new/episode.mkv")
            );
        }

        #[test]
        fn matches_by_decoded_url_segment_without_file_name() {
            let download = download(
                None,
                "https://cdn.example.com/old-token/Yakuza.Fianc%C3%A9.S01E05.mkv",
            );
            let infos = [info(
                Some("Yakuza.Fiancé.S01E05.mkv"),
                "https://cdn.example.com/new-token/Yakuza.Fianc%C3%A9.S01E05.mkv",
            )];
            assert_eq!(
                matched(&infos, &download),
                Some("https://cdn.example.com/new-token/Yakuza.Fianc%C3%A9.S01E05.mkv")
            );
        }

        #[test]
        fn single_file_torrent_returns_the_only_info() {
            let download = download(None, "https://cdn.example.com/old-token/movie.mkv");
            let infos = [info(
                Some("movie.mkv"),
                "https://cdn.example.com/new-token/movie.mkv",
            )];
            assert_eq!(
                matched(&infos, &download),
                Some("https://cdn.example.com/new-token/movie.mkv")
            );
        }

        #[test]
        fn several_files_without_match_return_none() {
            let download = download(None, "https://cdn.example.com/old-token/unknown.mkv");
            let infos = [
                info(Some("a.mkv"), "https://cdn.example.com/new/a.mkv"),
                info(Some("b.mkv"), "https://cdn.example.com/new/b.mkv"),
            ];
            assert_eq!(matched(&infos, &download), None);
        }

        #[test]
        fn unchanged_own_link_wins_over_duplicate_file_names() {
            let download = download(Some("eboot.bin"), "https://alldebrid.com/f/second");
            let infos = [
                info(Some("eboot.bin"), "https://alldebrid.com/f/first"),
                info(Some("eboot.bin"), "https://alldebrid.com/f/second"),
            ];
            assert_eq!(
                matched(&infos, &download),
                Some("https://alldebrid.com/f/second")
            );
        }

        #[test]
        fn duplicate_file_names_with_refreshed_links_return_none() {
            let download = download(Some("eboot.bin"), "https://alldebrid.com/f/old-second");
            let infos = [
                info(Some("eboot.bin"), "https://alldebrid.com/f/new-first"),
                info(Some("eboot.bin"), "https://alldebrid.com/f/new-second"),
            ];
            assert_eq!(matched(&infos, &download), None);
        }

        #[test]
        fn duplicate_url_segments_without_file_name_return_none() {
            let download = download(None, "https://cdn.example.com/old-token/sub/eboot.bin");
            let infos = [
                info(None, "https://cdn.example.com/new-token/eboot.bin"),
                info(None, "https://cdn.example.com/new-token/sub/eboot.bin"),
            ];
            assert_eq!(matched(&infos, &download), None);
        }

        #[test]
        fn no_infos_return_none() {
            assert_eq!(matched(&[], &download(None, "https://x/a.mkv")), None);
        }

        #[test]
        fn file_names_compare_case_insensitively() {
            let download = download(Some("ÉPISODE.MKV"), "https://x/old");
            let infos = [
                info(Some("épisode.mkv"), "https://x/new"),
                info(Some("other.mkv"), "https://x/other"),
            ];
            assert_eq!(matched(&infos, &download), Some("https://x/new"));
        }

        #[test]
        fn blank_names_never_match() {
            let download = download(Some(" "), "https://cdn.example.com/dir/");
            let infos = [
                info(Some(" "), "https://cdn.example.com/new/a.mkv"),
                info(None, "https://cdn.example.com/new/b.mkv"),
            ];
            assert_eq!(matched(&infos, &download), None);
        }

        #[test]
        fn plain_paths_match_by_their_last_component() {
            let download = download(None, "folder/movie.mkv");
            let infos = [
                info(Some("movie.mkv"), "highway://1/svc/2"),
                info(Some("other.mkv"), "highway://1/svc/3"),
            ];
            assert_eq!(matched(&infos, &download), Some("highway://1/svc/2"));
        }
    }
}
