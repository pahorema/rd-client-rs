//! Where downloads land on disk and how provider names become safe file
//! and directory names.

use url::Url;

use crate::Download;

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
        use super::super::*;

        fn download(link: Option<&str>, file_name: Option<&str>) -> Download {
            Download {
                link: link.map(str::to_owned),
                file_name: file_name.map(str::to_owned),
                ..Download::default()
            }
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
    }
}
