//! Multi-volume archive naming.
//!
//! Three naming schemes are recognised, case-insensitively:
//! - `name.part1.rar`, `name.part2.rar`, … (zero padding of the first number is kept);
//! - `name.rar`, `name.r00`, `name.r01`, … `name.r99`, `name.s00`, … `name.y99`, as `UnRAR`
//!   names old-style volumes past `.r99`; `.zNN` is left out because split ZIP archives use
//!   `name.z01`, `name.z02`, …;
//! - `name.001`, `name.002`, … (at least three digits).
//!
//! Names built from a volume name keep its stem and spell the volume part (`.part1.rar`,
//! `.r00`) in lower case; files on disk are matched ignoring the case of that part.

use std::fs;
use std::path::{Path, PathBuf};

/// Last extension letter of an old-style volume name (`.y99`); see the module documentation.
const LAST_LEGACY_LETTER: u8 = b'y';

/// Whether `path` is named like a volume of a multi-volume set other than its first one.
///
/// Such a file is unpacked together with the first volume of its set and must not be unpacked
/// on its own. The check uses the file name only.
pub fn is_non_first_volume(path: &Path) -> bool {
    parse(path).is_some_and(|volume| volume.index > 0)
}

/// Path of the first volume of the multi-volume set `path` is named after.
///
/// Every volume of a set maps to the same path, whatever the case of its volume part, so two
/// downloads belong to the same set exactly when their first volumes are equal. That path
/// spells the volume part in lower case (`Show.R00` and `Show.Rar` give `Show.rar`); [`unpack`]
/// finds the file whatever the case of that part. A name not shaped like a volume is returned
/// as is. The check uses the file name only.
///
/// [`unpack`]: crate::unpack
pub fn first_volume(path: &Path) -> PathBuf {
    parse(path)
        .and_then(|volume| volume.path(0))
        .unwrap_or_else(|| path.to_path_buf())
}

/// Existing volumes of the set `path` belongs to, in order, starting at its first volume.
///
/// Volumes are looked up on disk one after the other, ignoring the case of their volume part,
/// and the list stops at the first missing one. A file not named like a volume yields just
/// itself. The list is empty when the first volume does not exist.
pub fn volume_paths(path: &Path) -> Vec<PathBuf> {
    let Some(volume) = parse(path) else {
        return if path.is_file() {
            vec![path.to_path_buf()]
        } else {
            Vec::new()
        };
    };
    let siblings = sibling_files(path);
    (0..)
        .map_while(|index| volume.path(index))
        .map_while(|candidate| volume.find(&candidate, &siblings))
        .collect()
}

/// The file on disk that `path` names: `path` itself when it exists, otherwise, for a volume
/// name, the file whose name differs from it only in the case of the volume part.
pub(crate) fn locate(path: &Path) -> Option<PathBuf> {
    if path.is_file() {
        return Some(path.to_path_buf());
    }
    let volume = parse(path)?;
    volume.find(path, &sibling_files(path))
}

/// Names of the files in the directory of `path` (symbolic links to files included).
fn sibling_files(path: &Path) -> Vec<String> {
    let dir = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_file())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect()
}

/// A file name split into the parts that stay the same across a set and its volume index.
#[derive(Debug)]
struct VolumeName<'a> {
    path: &'a Path,
    /// File name up to and excluding the varying part.
    stem: &'a str,
    scheme: Scheme,
    /// Zero-based position in the set.
    index: u32,
}

#[derive(Debug)]
enum Scheme {
    /// `<stem>.part<number>.rar`, the number padded to `width` digits.
    Part { width: usize },
    /// `<stem>.rar`, then `<stem>.r00` ….
    Legacy,
    /// `<stem>.<number>` with `width` digits.
    Numeric { width: usize },
}

fn parse(path: &Path) -> Option<VolumeName<'_>> {
    let name = path.file_name()?.to_str()?;
    parse_part(path, name)
        .or_else(|| parse_legacy(path, name))
        .or_else(|| parse_numeric(path, name))
}

fn parse_part<'a>(path: &'a Path, name: &'a str) -> Option<VolumeName<'a>> {
    let (rest, ext) = split_ext(name)?;
    if !ext.eq_ignore_ascii_case(".rar") {
        return None;
    }
    let digits_start = rest.trim_end_matches(|c: char| c.is_ascii_digit()).len();
    let (rest, digits) = rest.split_at(digits_start);
    let part_start = rest.len().checked_sub(".part".len())?;
    let (stem, part) = rest.split_at_checked(part_start)?;
    if digits.is_empty() || !part.eq_ignore_ascii_case(".part") {
        return None;
    }
    let number: u32 = digits.parse().ok()?;
    Some(VolumeName {
        path,
        stem,
        scheme: Scheme::Part {
            width: digits.len(),
        },
        index: number.checked_sub(1)?,
    })
}

fn parse_legacy<'a>(path: &'a Path, name: &'a str) -> Option<VolumeName<'a>> {
    let (stem, ext) = split_ext(name)?;
    let ext = ext.strip_prefix('.')?;
    let index = if ext.eq_ignore_ascii_case("rar") {
        0
    } else {
        let &[letter, tens, ones] = ext.as_bytes() else {
            return None;
        };
        let letter = letter.to_ascii_lowercase();
        if !(b'r'..=LAST_LEGACY_LETTER).contains(&letter)
            || !tens.is_ascii_digit()
            || !ones.is_ascii_digit()
        {
            return None;
        }
        u32::from(letter - b'r') * 100 + u32::from(tens - b'0') * 10 + u32::from(ones - b'0') + 1
    };
    Some(VolumeName {
        path,
        stem,
        scheme: Scheme::Legacy,
        index,
    })
}

fn parse_numeric<'a>(path: &'a Path, name: &'a str) -> Option<VolumeName<'a>> {
    let (stem, ext) = split_ext(name)?;
    let digits = ext.strip_prefix('.')?;
    if digits.len() < 3 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let number: u32 = digits.parse().ok()?;
    Some(VolumeName {
        path,
        stem,
        scheme: Scheme::Numeric {
            width: digits.len(),
        },
        index: number.checked_sub(1)?,
    })
}

/// Splits `name` before its last `.`; the extension keeps the dot. Names starting with their
/// only dot have no extension.
fn split_ext(name: &str) -> Option<(&str, &str)> {
    let dot = name.rfind('.').filter(|&dot| dot > 0)?;
    Some(name.split_at(dot))
}

impl VolumeName<'_> {
    /// Path of the volume at zero-based `index` of this set, if the scheme can name it.
    fn path(&self, index: u32) -> Option<PathBuf> {
        let stem = self.stem;
        let name = match self.scheme {
            Scheme::Part { width } => {
                let number = index.checked_add(1)?;
                format!("{stem}.part{number:0width$}.rar")
            }
            Scheme::Legacy => {
                let ext = if index == 0 {
                    "rar".to_owned()
                } else {
                    let position = index - 1;
                    let letter = u8::try_from(position / 100)
                        .ok()
                        .and_then(|offset| b'r'.checked_add(offset))
                        .filter(|letter| *letter <= LAST_LEGACY_LETTER)?;
                    format!("{}{:02}", char::from(letter), position % 100)
                };
                format!("{stem}.{ext}")
            }
            Scheme::Numeric { width } => {
                let number = index.checked_add(1)?;
                format!("{stem}.{number:0width$}")
            }
        };
        Some(self.path.with_file_name(name))
    }

    /// The file among the sibling `files` that `candidate`, a path built by [`Self::path`],
    /// names: an exact match first, otherwise one differing only in the case of the volume part.
    fn find(&self, candidate: &Path, files: &[String]) -> Option<PathBuf> {
        let wanted = candidate.file_name()?.to_str()?;
        let (stem, volume_part) = wanted.split_at_checked(self.stem.len())?;
        let same_volume = |name: &&String| {
            name.strip_prefix(stem)
                .is_some_and(|rest| rest.eq_ignore_ascii_case(volume_part))
        };
        let found = files
            .iter()
            .find(|name| *name == wanted)
            .or_else(|| files.iter().find(same_volume))?;
        Some(candidate.with_file_name(found))
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn first_volumes_are_recognised() {
        for name in [
            "show.part1.rar",
            "show.part01.rar",
            "Show.PART001.RAR",
            "show.rar",
            "show.001",
            "plain.mkv",
            "noext",
        ] {
            assert!(!is_non_first_volume(Path::new(name)), "{name}");
        }
    }

    #[test]
    fn later_volumes_are_recognised() {
        for name in [
            "show.part2.rar",
            "show.part02.rar",
            "Show.Part10.RAR",
            "show.r00",
            "show.R01",
            "show.s05",
            "show.002",
            "show.mkv.0010",
        ] {
            assert!(is_non_first_volume(Path::new(name)), "{name}");
        }
    }

    #[test]
    fn first_volume_maps_every_scheme_to_its_first_name() {
        let cases = [
            ("dir/show.part07.rar", "dir/show.part01.rar"),
            ("dir/Show.Part3.Rar", "dir/Show.part1.rar"),
            ("dir/show.r00", "dir/show.rar"),
            ("dir/SHOW.R42", "dir/SHOW.rar"),
            ("dir/Show.RAR", "dir/Show.rar"),
            ("dir/show.s00", "dir/show.rar"),
            ("dir/show.007", "dir/show.001"),
            ("dir/show.rar", "dir/show.rar"),
            ("dir/show.mkv", "dir/show.mkv"),
        ];
        for (name, first) in cases {
            assert_eq!(first_volume(Path::new(name)), Path::new(first), "{name}");
        }
    }

    #[test]
    fn volumes_differing_in_case_share_their_first_volume() {
        for set in [
            ["dir/Show.Rar", "dir/Show.r00", "dir/Show.R01"],
            [
                "dir/Show.part1.rar",
                "dir/Show.PART2.RAR",
                "dir/Show.Part3.Rar",
            ],
        ] {
            let firsts: Vec<PathBuf> = set
                .iter()
                .map(|name| first_volume(Path::new(name)))
                .collect();
            assert!(firsts.iter().all(|first| *first == firsts[0]), "{firsts:?}");
        }
    }

    #[test]
    fn volumes_are_found_on_disk_whatever_the_case_of_their_volume_part() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["Show.Rar", "Show.R00", "Show.r01"] {
            fs::write(dir.path().join(name), name).unwrap();
        }

        let paths = volume_paths(&dir.path().join("Show.r01"));
        let located = locate(&first_volume(&dir.path().join("Show.R00"))).unwrap();

        let expected: Vec<PathBuf> = ["Show.Rar", "Show.R00", "Show.r01"]
            .iter()
            .map(|name| dir.path().join(name))
            .collect();
        assert_eq!(paths, expected);
        assert_eq!(fs::read_to_string(located).unwrap(), "Show.Rar");
        assert_eq!(locate(&dir.path().join("Other.rar")), None);
    }

    #[test]
    fn legacy_names_roll_the_letter_after_one_hundred_volumes() {
        let volume = parse(Path::new("a.rar")).unwrap();
        assert_eq!(volume.path(1).unwrap(), Path::new("a.r00"));
        assert_eq!(volume.path(100).unwrap(), Path::new("a.r99"));
        assert_eq!(volume.path(101).unwrap(), Path::new("a.s00"));
        assert_eq!(parse(Path::new("a.s00")).unwrap().index, 101);
        assert_eq!(volume.path(800).unwrap(), Path::new("a.y99"));
        assert!(volume.path(801).is_none());
    }

    #[test]
    fn split_zip_parts_are_not_taken_for_rar_volumes() {
        for name in ["archive.z01", "archive.Z02", "archive.z99"] {
            assert!(!is_non_first_volume(Path::new(name)), "{name}");
            assert_eq!(first_volume(Path::new(name)), Path::new(name), "{name}");
        }
    }

    #[test]
    fn part_numbers_grow_past_their_padding() {
        let volume = parse(Path::new("a.part9.rar")).unwrap();
        assert_eq!(volume.path(9).unwrap(), Path::new("a.part10.rar"));
    }

    #[test]
    fn volume_paths_lists_consecutive_existing_volumes_from_the_first() {
        let dir = tempfile::tempdir().unwrap();
        for name in [
            "set.part1.rar",
            "set.part2.rar",
            "set.part3.rar",
            "set.part5.rar",
        ] {
            fs::write(dir.path().join(name), b"x").unwrap();
        }

        let paths = volume_paths(&dir.path().join("set.part2.rar"));

        let expected: Vec<PathBuf> = ["set.part1.rar", "set.part2.rar", "set.part3.rar"]
            .iter()
            .map(|name| dir.path().join(name))
            .collect();
        assert_eq!(paths, expected);
    }

    #[test]
    fn volume_paths_follows_legacy_and_numeric_names() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["old.rar", "old.r00", "old.r01", "split.001", "split.002"] {
            fs::write(dir.path().join(name), b"x").unwrap();
        }

        assert_eq!(volume_paths(&dir.path().join("old.r01")).len(), 3);
        assert_eq!(volume_paths(&dir.path().join("split.001")).len(), 2);
    }

    #[test]
    fn volume_paths_of_a_plain_file_is_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("movie.mkv");
        fs::write(&file, b"x").unwrap();

        assert_eq!(volume_paths(&file), vec![file]);
        assert_eq!(
            volume_paths(&dir.path().join("missing.part2.rar")),
            Vec::<PathBuf>::new()
        );
    }
}
