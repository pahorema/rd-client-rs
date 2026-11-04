//! ZIP listing and extraction.

use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use zip::ZipArchive;

use crate::Error;
use crate::extract::Entry;

type Zip = ZipArchive<BufReader<File>>;

fn open(archive: &Path) -> Result<Zip, Error> {
    let file = File::open(archive).map_err(Error::io("open", archive))?;
    ZipArchive::new(BufReader::new(file)).map_err(Error::read(archive))
}

/// Lists the file entries of `archive`, skipping directories.
pub(crate) fn list(archive: &Path) -> Result<Vec<Entry>, Error> {
    let mut zip = open(archive)?;
    let mut entries = Vec::with_capacity(zip.len());
    for index in 0..zip.len() {
        let file = zip.by_index_raw(index).map_err(Error::read(archive))?;
        if file.is_dir() {
            continue;
        }
        let name = file.name().map_err(Error::read(archive))?.into_owned();
        entries.push(Entry {
            name,
            size: file.size(),
        });
    }
    Ok(entries)
}
