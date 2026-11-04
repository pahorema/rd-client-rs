//! Splitting a file into byte ranges fetched by separate connections.

/// Files up to this size are always fetched as one chunk.
const MIN_SIZE_FOR_CHUNKING: u64 = 512;

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

/// Splits a file of `size` bytes (`None` when unknown) into chunks.
///
/// A single chunk is used when the size is unknown, at most 512 bytes, or the server does not
/// serve byte ranges. Otherwise `chunk_count` is clamped to `1..=size`, every chunk gets
/// `size / count` bytes and the last one also takes the remainder.
pub(crate) fn plan_chunks(
    size: Option<u64>,
    supports_ranges: bool,
    chunk_count: u64,
) -> Vec<ChunkRange> {
    let Some(size) = size.filter(|size| *size > 0) else {
        return vec![ChunkRange {
            start: 0,
            end: None,
        }];
    };
    let count = if !supports_ranges || size <= MIN_SIZE_FOR_CHUNKING {
        1
    } else {
        chunk_count.clamp(1, size)
    };
    let chunk_size = size / count;
    (0..count)
        .map(|index| {
            let start = index * chunk_size;
            let end = if index == count - 1 {
                size - 1
            } else {
                start + chunk_size - 1
            };
            ChunkRange {
                start,
                end: Some(end),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ranges(chunks: &[ChunkRange]) -> Vec<(u64, Option<u64>)> {
        chunks
            .iter()
            .map(|chunk| (chunk.start, chunk.end))
            .collect()
    }

    #[test]
    fn splits_evenly_divisible_size() {
        let chunks = plan_chunks(Some(4000), true, 4);
        assert_eq!(
            ranges(&chunks),
            vec![
                (0, Some(999)),
                (1000, Some(1999)),
                (2000, Some(2999)),
                (3000, Some(3999))
            ]
        );
    }

    #[test]
    fn last_chunk_absorbs_the_remainder() {
        let chunks = plan_chunks(Some(1003), true, 4);
        assert_eq!(
            ranges(&chunks),
            vec![
                (0, Some(249)),
                (250, Some(499)),
                (500, Some(749)),
                (750, Some(1002))
            ]
        );
        let total: u64 = chunks.iter().filter_map(|chunk| chunk.len()).sum();
        assert_eq!(total, 1003);
    }

    #[test]
    fn small_files_use_one_chunk() {
        assert_eq!(
            ranges(&plan_chunks(Some(512), true, 8)),
            vec![(0, Some(511))]
        );
        assert_eq!(plan_chunks(Some(513), true, 8).len(), 8);
    }

    #[test]
    fn unknown_size_reads_one_open_ended_chunk() {
        assert_eq!(ranges(&plan_chunks(None, true, 8)), vec![(0, None)]);
        assert_eq!(ranges(&plan_chunks(Some(0), true, 8)), vec![(0, None)]);
    }

    #[test]
    fn missing_range_support_uses_one_chunk() {
        assert_eq!(
            ranges(&plan_chunks(Some(10_000), false, 8)),
            vec![(0, Some(9999))]
        );
    }

    #[test]
    fn chunk_count_never_exceeds_size() {
        let chunks = plan_chunks(Some(600), true, 1000);
        assert_eq!(chunks.len(), 600);
        assert!(chunks.iter().all(|chunk| chunk.len() == Some(1)));
    }

    #[test]
    fn chunks_cover_the_file_without_gaps() {
        for (size, count) in [(513, 7), (1_000_003, 8), (65_536, 3)] {
            let chunks = plan_chunks(Some(size), true, count);
            let mut next = 0;
            for chunk in &chunks {
                assert_eq!(chunk.start, next, "size {size} count {count}");
                next = chunk.end.expect("bounded") + 1;
            }
            assert_eq!(next, size, "size {size} count {count}");
        }
    }
}
