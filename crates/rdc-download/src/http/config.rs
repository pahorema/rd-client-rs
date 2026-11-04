//! Configuration of the internal HTTP engine.

/// Buffer size used when the configured size is `0` or negative.
const DEFAULT_BUFFER_SIZE: usize = 64 * 1024;
/// Smallest buffer used for reads and writes.
const MIN_BUFFER_SIZE: usize = 4 * 1024;
/// Largest buffer used per connection, so many chunks cannot exhaust memory.
const MAX_BUFFER_SIZE: usize = 64 * 1024 * 1024;

/// Settings of the internal HTTP engine, re-read live while a download runs.
///
/// Integer fields take the raw values of the settings they come from; out-of-range values
/// are normalised as documented on each field. The buffer size applies to connections opened
/// afterwards.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InternalConfig {
    /// Read/write buffer of each connection in bytes; `0` or less means 64 KiB.
    pub buffer_size: i32,
}

impl InternalConfig {
    /// Creates the configuration with every value at its normalised default.
    pub fn new() -> Self {
        Self::default()
    }

    pub(crate) fn effective_buffer_size(&self) -> usize {
        usize::try_from(self.buffer_size)
            .ok()
            .filter(|size| *size > 0)
            .unwrap_or(DEFAULT_BUFFER_SIZE)
            .clamp(MIN_BUFFER_SIZE, MAX_BUFFER_SIZE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffer_size_is_defaulted_and_bounded() {
        let buffer = |buffer_size| InternalConfig { buffer_size }.effective_buffer_size();
        assert_eq!(buffer(0), 64 * 1024);
        assert_eq!(buffer(10), 4 * 1024);
        assert_eq!(buffer(4 * 1024 * 1024), 4 * 1024 * 1024);
        assert_eq!(buffer(i32::MAX), 64 * 1024 * 1024);
    }
}
