//! Post-download check of a file's size against the size the provider announced.

use std::io;
use std::path::{Path, PathBuf};

/// Smallest accepted deviation from the expected size, in bytes.
const MIN_TOLERANCE: u64 = 64 * 1024;

/// Why a downloaded file failed the size check; the message is shown to the user.
#[derive(Debug, thiserror::Error)]
pub enum VerifyFileSizeError {
    /// Nothing exists at the downloaded path.
    #[error("Downloaded file was not found at {} after the download reported success.", .path.display())]
    Missing {
        /// The checked path.
        path: PathBuf,
    },
    /// The size deviates from the expected one by more than the tolerance.
    #[error(
        "Downloaded file size ({} bytes) is outside the expected range ({} bytes ±{}).",
        group_digits(*.actual),
        group_digits(*.expected),
        group_digits(*.tolerance)
    )]
    OutOfRange {
        /// Size found on disk.
        actual: u64,
        /// Size the provider announced.
        expected: u64,
        /// Accepted deviation.
        tolerance: u64,
    },
    /// The file could not be inspected.
    #[error("Failed to verify the size of the downloaded file at {}: {source}", .path.display())]
    Io {
        /// The checked path.
        path: PathBuf,
        /// The underlying failure.
        #[source]
        source: io::Error,
    },
}

/// Checks that the file at `path` has about `expected` bytes.
///
/// The accepted deviation is the larger of 64 KiB and 1 % of `expected`. An `expected` of `0`
/// means "unknown" and always passes, as does an empty path.
pub fn verify_file_size(path: &Path, expected: u64) -> Result<(), VerifyFileSizeError> {
    if expected == 0 || path.as_os_str().is_empty() {
        return Ok(());
    }
    let actual = match std::fs::metadata(path) {
        Ok(metadata) => metadata.len(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(VerifyFileSizeError::Missing {
                path: path.to_owned(),
            });
        }
        Err(source) => {
            return Err(VerifyFileSizeError::Io {
                path: path.to_owned(),
                source,
            });
        }
    };
    let tolerance = MIN_TOLERANCE.max(expected / 100);
    if actual.abs_diff(expected) > tolerance {
        return Err(VerifyFileSizeError::OutOfRange {
            actual,
            expected,
            tolerance,
        });
    }
    Ok(())
}

/// Formats `value` with `,` between groups of three digits.
fn group_digits(value: u64) -> String {
    let digits = value.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file_of(len: u64) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("file.bin");
        let file = std::fs::File::create(&path).expect("create");
        file.set_len(len).expect("set len");
        (dir, path)
    }

    #[test]
    fn accepts_exact_size() {
        let (_dir, path) = file_of(1000);
        assert!(verify_file_size(&path, 1000).is_ok());
    }

    #[test]
    fn accepts_deviation_within_minimum_tolerance() {
        let (_dir, path) = file_of(100_000);
        assert!(verify_file_size(&path, 100_000 + 65_536).is_ok());
        assert!(verify_file_size(&path, 100_000 + 65_537).is_err());
    }

    #[test]
    fn tolerance_grows_to_one_percent_of_large_files() {
        let (_dir, path) = file_of(10_000_000);
        assert!(verify_file_size(&path, 10_100_000).is_ok());
        let error = verify_file_size(&path, 10_102_100).expect_err("outside tolerance");
        assert_eq!(
            error.to_string(),
            "Downloaded file size (10,000,000 bytes) is outside the expected range (10,102,100 bytes ±101,021)."
        );
    }

    #[test]
    fn reports_missing_file() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("absent.bin");
        let error = verify_file_size(&path, 10).expect_err("missing");
        assert_eq!(
            error.to_string(),
            format!(
                "Downloaded file was not found at {} after the download reported success.",
                path.display()
            )
        );
    }

    #[test]
    fn unknown_expected_size_or_empty_path_passes() {
        assert!(verify_file_size(Path::new("/definitely/not/here"), 0).is_ok());
        assert!(verify_file_size(Path::new(""), 10).is_ok());
    }

    #[test]
    fn groups_digits_by_thousands() {
        assert_eq!(group_digits(0), "0");
        assert_eq!(group_digits(999), "999");
        assert_eq!(group_digits(1000), "1,000");
        assert_eq!(group_digits(65_536), "65,536");
        assert_eq!(group_digits(1_234_567_890), "1,234,567,890");
    }
}
