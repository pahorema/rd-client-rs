//! Configuration of the internal HTTP engine.

use std::time::Duration;

/// Chunk count used when the configured count is `0` or negative.
const DEFAULT_CHUNK_COUNT: u64 = 8;
/// Per-read timeout used when the configured timeout is `0` or negative.
const DEFAULT_TIMEOUT_MS: u64 = 1000;
/// Shortest accepted per-read timeout.
const MIN_TIMEOUT_MS: u64 = 100;
/// Shortest wait for response headers.
const MIN_HEADER_TIMEOUT_MS: u64 = 1000;
/// Buffer size used when the configured size is `0` or negative.
const DEFAULT_BUFFER_SIZE: usize = 64 * 1024;
/// Smallest buffer used for reads and writes.
const MIN_BUFFER_SIZE: usize = 4 * 1024;
/// Largest buffer used per connection, so many chunks cannot exhaust memory.
const MAX_BUFFER_SIZE: usize = 64 * 1024 * 1024;

/// Settings of the internal HTTP engine, re-read live while a download runs.
///
/// Integer fields take the raw values of the settings they come from; out-of-range values
/// are normalised as documented on each field. Speed and connection limits apply to running
/// downloads immediately; the chunk count, timeout and buffer size apply to downloads and
/// connections started afterwards; the proxy applies to new connections.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InternalConfig {
    /// Speed limit of one download in bytes per second, shared by its connections; `0` means
    /// unlimited.
    pub max_speed: u64,
    /// Maximum concurrent connections per download; `1` or less means one connection.
    pub parallel_connections: i32,
    /// Number of chunks a file is split into; `0` or less means 8. Files that are small, of
    /// unknown size or served without range support are fetched as one chunk.
    pub chunk_count: i32,
    /// Per-read timeout in milliseconds; `0` or less means 1000, at least 100.
    pub timeout_ms: i32,
    /// Read/write buffer of each connection in bytes; `0` or less means 64 KiB.
    pub buffer_size: i32,
    /// Proxy URL every request goes through; blank means a direct connection.
    pub proxy: Option<String>,
}

impl InternalConfig {
    /// Creates the configuration with every value at its normalised default.
    pub fn new() -> Self {
        Self::default()
    }

    pub(crate) fn effective_parallel_connections(&self) -> usize {
        usize::try_from(self.parallel_connections)
            .unwrap_or(1)
            .max(1)
    }

    pub(crate) fn effective_chunk_count(&self) -> u64 {
        u64::try_from(self.chunk_count)
            .ok()
            .filter(|count| *count > 0)
            .unwrap_or(DEFAULT_CHUNK_COUNT)
    }

    pub(crate) fn read_timeout(&self) -> Duration {
        let millis = u64::try_from(self.timeout_ms)
            .ok()
            .filter(|millis| *millis > 0)
            .unwrap_or(DEFAULT_TIMEOUT_MS)
            .max(MIN_TIMEOUT_MS);
        Duration::from_millis(millis)
    }

    /// How long to wait for response headers.
    pub(crate) fn header_timeout(&self) -> Duration {
        self.read_timeout()
            .max(Duration::from_millis(MIN_HEADER_TIMEOUT_MS))
    }

    pub(crate) fn effective_buffer_size(&self) -> usize {
        usize::try_from(self.buffer_size)
            .ok()
            .filter(|size| *size > 0)
            .unwrap_or(DEFAULT_BUFFER_SIZE)
            .clamp(MIN_BUFFER_SIZE, MAX_BUFFER_SIZE)
    }

    pub(crate) fn proxy_url(&self) -> Option<&str> {
        self.proxy
            .as_deref()
            .map(str::trim)
            .filter(|proxy| !proxy.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_or_one_parallel_connections_mean_one_connection() {
        for value in [-3, 0, 1] {
            let config = InternalConfig {
                parallel_connections: value,
                ..InternalConfig::new()
            };
            assert_eq!(config.effective_parallel_connections(), 1, "value {value}");
        }
        let config = InternalConfig {
            parallel_connections: 6,
            ..InternalConfig::new()
        };
        assert_eq!(config.effective_parallel_connections(), 6);
    }

    #[test]
    fn non_positive_chunk_count_means_eight() {
        for value in [-1, 0] {
            let config = InternalConfig {
                chunk_count: value,
                ..InternalConfig::new()
            };
            assert_eq!(config.effective_chunk_count(), 8, "value {value}");
        }
        let config = InternalConfig {
            chunk_count: 3,
            ..InternalConfig::new()
        };
        assert_eq!(config.effective_chunk_count(), 3);
    }

    #[test]
    fn timeout_defaults_to_one_second_and_is_at_least_100_ms() {
        let timeout = |timeout_ms| {
            InternalConfig {
                timeout_ms,
                ..InternalConfig::new()
            }
            .read_timeout()
        };
        assert_eq!(timeout(0), Duration::from_millis(1000));
        assert_eq!(timeout(-5), Duration::from_millis(1000));
        assert_eq!(timeout(20), Duration::from_millis(100));
        assert_eq!(timeout(5000), Duration::from_millis(5000));
    }

    #[test]
    fn header_timeout_is_at_least_one_second() {
        let mut config = InternalConfig {
            timeout_ms: 200,
            ..InternalConfig::new()
        };
        assert_eq!(config.header_timeout(), Duration::from_millis(1000));
        config.timeout_ms = 3000;
        assert_eq!(config.header_timeout(), Duration::from_millis(3000));
    }

    #[test]
    fn buffer_size_is_defaulted_and_bounded() {
        let buffer = |buffer_size| {
            InternalConfig {
                buffer_size,
                ..InternalConfig::new()
            }
            .effective_buffer_size()
        };
        assert_eq!(buffer(0), 64 * 1024);
        assert_eq!(buffer(10), 4 * 1024);
        assert_eq!(buffer(4 * 1024 * 1024), 4 * 1024 * 1024);
        assert_eq!(buffer(i32::MAX), 64 * 1024 * 1024);
    }

    #[test]
    fn blank_proxy_means_direct_connection() {
        let proxy = |value: Option<&str>| {
            InternalConfig {
                proxy: value.map(str::to_owned),
                ..InternalConfig::new()
            }
            .proxy_url()
            .map(str::to_owned)
        };
        assert_eq!(proxy(None), None);
        assert_eq!(proxy(Some("  ")), None);
        assert_eq!(
            proxy(Some(" http://proxy:8080 ")),
            Some("http://proxy:8080".to_owned())
        );
    }
}
