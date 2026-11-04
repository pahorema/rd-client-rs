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
