//! Helpers shared by the integration tests.

use std::fs::File;
use std::io::Write;
use std::path::Path;

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
