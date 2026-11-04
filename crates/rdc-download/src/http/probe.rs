//! Learning a remote file's size and whether its server serves byte ranges.

use std::time::Duration;

use reqwest::header::{
    ACCEPT_RANGES, CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_RANGE, HeaderMap, HeaderName,
};
use reqwest::{Response, StatusCode, Url};

use super::connection::{ByteRange, Context};
use super::error::{TransferError, is_transient_status};

/// Retries of a probe answered with a passing server error.
const MAX_PROBE_RETRIES: u32 = 3;
/// Longest wait between probe attempts.
const MAX_PROBE_RETRY_DELAY: Duration = Duration::from_secs(10);
/// The probe asks for the first byte only.
const FIRST_BYTE: ByteRange = ByteRange {
    start: 0,
    end: Some(0),
};

/// What the probe learned about the remote file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RemoteFile {
    /// Address after redirects; every chunk is fetched from here.
    pub(crate) url: Url,
    /// Size in bytes; `None` when unknown.
    pub(crate) size: Option<u64>,
    pub(crate) supports_ranges: bool,
}

/// Probes `url` with a `Range: bytes=0-0` request.
///
/// Servers that refuse the range request are asked again without it. Probes answered with a
/// passing server condition (throttling, overload) are retried up to three times.
pub(crate) async fn probe(context: &Context, url: &Url) -> Result<RemoteFile, TransferError> {
    let mut attempt = 0;
    loop {
        attempt += 1;
        match probe_once(context, url).await {
            Err(TransferError::Status(status))
                if is_transient_status(status)
                    && !status.is_redirection()
                    && attempt <= MAX_PROBE_RETRIES =>
            {
                let delay = (context.config().read_timeout() * attempt).min(MAX_PROBE_RETRY_DELAY);
                tracing::debug!("Probe of {url} answered {status}, retrying in {delay:?}");
                Context::sleep(&context.cancel, delay).await?;
            }
            result => return result,
        }
    }
}

async fn probe_once(context: &Context, url: &Url) -> Result<RemoteFile, TransferError> {
    let cancel = &context.cancel;
    match context.get(url, Some(FIRST_BYTE), cancel).await {
        Ok(response) => Ok(describe(&response)),
        Err(TransferError::Status(status))
            if status == StatusCode::RANGE_NOT_SATISFIABLE || !status.is_redirection() =>
        {
            // Some servers refuse range requests although the file downloads fine without one.
            let plain = describe(&context.get(url, None, cancel).await?);
            if !(status.is_server_error() || is_transient_status(status)) {
                return Ok(plain);
            }
            // A passing server error says nothing about range support: ask once more.
            match context.get(url, Some(FIRST_BYTE), cancel).await {
                Ok(response) => Ok(describe(&response)),
                Err(TransferError::Cancelled) => Err(TransferError::Cancelled),
                Err(_) => Ok(plain),
            }
        }
        Err(error) => Err(error),
    }
}

fn describe(response: &Response) -> RemoteFile {
    let (size, supports_ranges) = size_and_range_support(response.headers());
    RemoteFile {
        url: response.url().clone(),
        size,
        supports_ranges,
    }
}

/// Reads the file size and range support from probe response headers.
///
/// A compressed representation has neither a usable size nor usable ranges. Sizes of one
/// byte or less count as unknown.
fn size_and_range_support(headers: &HeaderMap) -> (Option<u64>, bool) {
    let text = |name: HeaderName| header_text(headers, &name);
    if text(CONTENT_ENCODING).is_some_and(|encoding| !encoding.eq_ignore_ascii_case("identity")) {
        return (None, false);
    }
    let refuses_ranges =
        text(ACCEPT_RANGES).is_some_and(|value| value.eq_ignore_ascii_case("none"));
    let content_range = text(CONTENT_RANGE);
    let supports_ranges = !refuses_ranges && content_range.is_some();
    let size = if supports_ranges {
        content_range.and_then(total_from_content_range)
    } else {
        text(CONTENT_LENGTH).and_then(|length| length.parse().ok())
    };
    (size.filter(|size| *size > 1), supports_ranges)
}

/// The trimmed, non-empty text of header `name`.
fn header_text<'map>(headers: &'map HeaderMap, name: &HeaderName) -> Option<&'map str> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

/// Extracts the complete length from a `Content-Range` value like `bytes 0-0/1234`.
fn total_from_content_range(value: &str) -> Option<u64> {
    let rest = value.trim().strip_prefix("bytes")?;
    let (_, total) = rest.split_once('/')?;
    total.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use reqwest::header::HeaderValue;

    use super::*;

    fn headers(pairs: &[(HeaderName, &str)]) -> HeaderMap {
        pairs
            .iter()
            .map(|(name, value)| {
                (
                    name.clone(),
                    HeaderValue::from_str(value).expect("header value"),
                )
            })
            .collect()
    }

    #[test]
    fn content_range_gives_size_and_range_support() {
        let map = headers(&[(CONTENT_RANGE, "bytes 0-0/12345"), (CONTENT_LENGTH, "1")]);
        assert_eq!(size_and_range_support(&map), (Some(12345), true));
    }

    #[test]
    fn plain_response_gives_content_length_without_ranges() {
        let map = headers(&[(CONTENT_LENGTH, "4096")]);
        assert_eq!(size_and_range_support(&map), (Some(4096), false));
    }

    #[test]
    fn accept_ranges_none_disables_ranges() {
        let map = headers(&[
            (ACCEPT_RANGES, "none"),
            (CONTENT_RANGE, "bytes 0-0/100"),
            (CONTENT_LENGTH, "100"),
        ]);
        assert_eq!(size_and_range_support(&map), (Some(100), false));
    }

    #[test]
    fn compressed_representation_has_unknown_size_and_no_ranges() {
        let map = headers(&[(CONTENT_ENCODING, "gzip"), (CONTENT_RANGE, "bytes 0-0/100")]);
        assert_eq!(size_and_range_support(&map), (None, false));
        let map = headers(&[(CONTENT_ENCODING, "identity"), (CONTENT_LENGTH, "100")]);
        assert_eq!(size_and_range_support(&map), (Some(100), false));
    }

    #[test]
    fn unknown_or_tiny_sizes_count_as_unknown() {
        assert_eq!(size_and_range_support(&headers(&[])), (None, false));
        let map = headers(&[(CONTENT_RANGE, "bytes 0-0/*")]);
        assert_eq!(size_and_range_support(&map), (None, true));
        let map = headers(&[(CONTENT_LENGTH, "1")]);
        assert_eq!(size_and_range_support(&map), (None, false));
    }

    #[test]
    fn parses_content_range_totals() {
        assert_eq!(total_from_content_range("bytes 0-0/77"), Some(77));
        assert_eq!(total_from_content_range("bytes 5 - 9 / 10"), Some(10));
        assert_eq!(total_from_content_range("bytes */42"), Some(42));
        assert_eq!(total_from_content_range("items 0-0/10"), None);
        assert_eq!(total_from_content_range("bytes 0-0"), None);
    }
}
