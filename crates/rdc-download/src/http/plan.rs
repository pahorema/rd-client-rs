//! Splitting a file into byte ranges fetched by separate connections.

/// One byte range of the file; `end` is inclusive and `None` for an open-ended read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ChunkRange {
    pub(crate) start: u64,
    pub(crate) end: Option<u64>,
}

impl ChunkRange {
    /// Number of bytes in the range; `None` when it is read until the stream ends.
    pub(crate) fn len(self) -> Option<u64> {
        self.end.map(|end| end - self.start + 1)
    }
}
