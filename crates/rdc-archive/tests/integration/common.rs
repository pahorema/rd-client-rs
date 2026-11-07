//! Helpers shared by the integration tests.

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

/// Writes a ZIP archive at `path`; names ending in `/` become directory entries.
pub(crate) fn write_zip(path: &Path, entries: &[(&str, &[u8])]) {
    let file = File::create(path).expect("zip fixture can be created");
    let mut zip = ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    for (name, content) in entries {
        if name.ends_with('/') {
            zip.add_directory(*name, options)
                .expect("zip directory entry can be added");
        } else {
            zip.start_file(*name, options)
                .expect("zip file entry can be added");
            zip.write_all(content).expect("zip entry can be written");
        }
    }
    zip.finish().expect("zip fixture can be finished");
}

/// Writes a ZIP archive at `path` with one stored entry whose headers declare `declared` bytes
/// of uncompressed data, whatever the length of `content`.
///
/// The bytes follow the local file header, central directory header and end of central
/// directory record layouts of the ZIP specification (APPNOTE.TXT, sections 4.3.7, 4.3.12 and
/// 4.3.16); the CRC is left at zero because the data is never read.
pub(crate) fn write_zip_declaring(path: &Path, name: &str, content: &[u8], declared: u32) {
    fn u16_le(value: usize) -> [u8; 2] {
        u16::try_from(value)
            .expect("fixture field fits 16 bits")
            .to_le_bytes()
    }
    fn u32_le(value: usize) -> [u8; 4] {
        u32::try_from(value)
            .expect("fixture field fits 32 bits")
            .to_le_bytes()
    }
    const VERSION: [u8; 2] = 20_u16.to_le_bytes();
    const STORED: [u8; 2] = [0, 0];
    const DOS_DATE: [u8; 4] = [0, 0, 0x21, 0];
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&0x0403_4b50_u32.to_le_bytes());
    bytes.extend_from_slice(&VERSION);
    bytes.extend_from_slice(&[0, 0]);
    bytes.extend_from_slice(&STORED);
    bytes.extend_from_slice(&DOS_DATE);
    bytes.extend_from_slice(&[0; 4]);
    bytes.extend_from_slice(&u32_le(content.len()));
    bytes.extend_from_slice(&declared.to_le_bytes());
    bytes.extend_from_slice(&u16_le(name.len()));
    bytes.extend_from_slice(&[0, 0]);
    bytes.extend_from_slice(name.as_bytes());
    bytes.extend_from_slice(content);
    let directory = bytes.len();
    bytes.extend_from_slice(&0x0201_4b50_u32.to_le_bytes());
    bytes.extend_from_slice(&VERSION);
    bytes.extend_from_slice(&VERSION);
    bytes.extend_from_slice(&[0, 0]);
    bytes.extend_from_slice(&STORED);
    bytes.extend_from_slice(&DOS_DATE);
    bytes.extend_from_slice(&[0; 4]);
    bytes.extend_from_slice(&u32_le(content.len()));
    bytes.extend_from_slice(&declared.to_le_bytes());
    bytes.extend_from_slice(&u16_le(name.len()));
    bytes.extend_from_slice(&[0; 12]);
    bytes.extend_from_slice(&[0; 4]);
    bytes.extend_from_slice(name.as_bytes());
    let directory_size = bytes.len() - directory;
    bytes.extend_from_slice(&0x0605_4b50_u32.to_le_bytes());
    bytes.extend_from_slice(&[0; 4]);
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&u32_le(directory_size));
    bytes.extend_from_slice(&u32_le(directory));
    bytes.extend_from_slice(&[0, 0]);
    fs::write(path, bytes).expect("zip fixture can be written");
}

/// Directory holding the committed RAR fixtures (see `tests/fixtures/generate.sh`).
pub(crate) fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// Copies the fixture files `names` into `dir` and returns the path of the first copy.
pub(crate) fn copy_fixtures(dir: &Path, names: &[&str]) -> PathBuf {
    copy_fixtures_as(
        dir,
        &names.iter().map(|name| (*name, *name)).collect::<Vec<_>>(),
    )
}

/// Copies each fixture `(source, copy)` to `dir/copy` and returns the path of the first copy.
pub(crate) fn copy_fixtures_as(dir: &Path, names: &[(&str, &str)]) -> PathBuf {
    for (source, copy) in names {
        fs::copy(fixtures().join(source), dir.join(copy)).expect("fixture can be copied");
    }
    dir.join(names[0].1)
}

/// Content of `big.txt` in the RAR fixtures: 1500 numbered lines.
pub(crate) fn big_text() -> String {
    use std::fmt::Write as _;
    (0..1500).fold(String::new(), |mut text, line| {
        writeln!(text, "line {line:05}").expect("writing to a String cannot fail");
        text
    })
}

/// Every file below `dir`, as `/`-separated paths relative to `dir`, sorted.
pub(crate) fn files_below(dir: &Path) -> Vec<String> {
    fn walk(root: &Path, dir: &Path, files: &mut Vec<String>) {
        for entry in fs::read_dir(dir).expect("directory is readable") {
            let path = entry.expect("directory entry is readable").path();
            if path.is_dir() {
                walk(root, &path, files);
            } else {
                let parts: Vec<_> = path
                    .strip_prefix(root)
                    .expect("walked path is below the root")
                    .components()
                    .map(|part| part.as_os_str().to_string_lossy().into_owned())
                    .collect();
                files.push(parts.join("/"));
            }
        }
    }
    let mut files = Vec::new();
    if dir.exists() {
        walk(dir, dir, &mut files);
    }
    files.sort();
    files
}
