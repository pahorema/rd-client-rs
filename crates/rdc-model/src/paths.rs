//! Where downloads land on disk and how provider names become safe file
//! and directory names.

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
}
