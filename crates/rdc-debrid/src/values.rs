//! Small conversions of values read from provider answers.

/// Longest part of a response body quoted in error messages.
const BODY_EXCERPT_CHARS: usize = 300;

/// The start of `body` (at most 300 characters), for quoting an unexpected
/// response in an error message.
pub(crate) fn excerpt(body: &str) -> &str {
    body.char_indices()
        .nth(BODY_EXCERPT_CHARS)
        .map_or(body, |(end, _)| &body[..end])
}

/// `text` unless it is missing, empty or only whitespace.
pub(crate) fn non_blank(text: Option<&str>) -> Option<&str> {
    text.filter(|text| !text.trim().is_empty())
}

/// The last component of a provider file path (`/` or `\` separated).
pub(crate) fn base_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// A completion fraction between 0 and 1 as a whole percentage.
#[expect(
    clippy::cast_possible_truncation,
    reason = "a fraction of 1 times 100 always fits; NaN and overflow saturate"
)]
pub(crate) fn percent(fraction: f64) -> i64 {
    (fraction * 100.0) as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn excerpts_keep_at_most_300_characters() {
        let long = "é".repeat(400);

        assert_eq!(excerpt(&long).chars().count(), 300);
        assert_eq!(excerpt("short"), "short");
    }

    #[test]
    fn base_names_follow_either_separator() {
        assert_eq!(base_name("Release/Sub/file.mkv"), "file.mkv");
        assert_eq!(base_name(r"Release\file.mkv"), "file.mkv");
        assert_eq!(base_name("file.mkv"), "file.mkv");
    }
}
