//! Builds RAR 5 archives byte by byte for entries the committed fixtures do not cover.
//!
//! The layout follows the RAR 5.0 archive format description (<https://www.rarlab.com/technote.htm>):
//! the signature, then blocks made of a CRC32 of the header, the header size as a variable-length
//! integer, the header fields and an optional data area. Entries are stored (method 0), so their
//! data area is their content.

use std::fs;
use std::path::Path;

/// `Rar!\x1A\x07\x01\x00`: the signature of a RAR 5 archive.
const SIGNATURE: &[u8] = b"Rar!\x1a\x07\x01\x00";

const HEAD_MAIN: u64 = 1;
const HEAD_FILE: u64 = 2;
const HEAD_END: u64 = 5;

/// Header flags: an extra area / a data area follows the common fields.
const HFL_EXTRA: u64 = 0x1;
const HFL_DATA: u64 = 0x2;

/// File flag: the data CRC32 is present.
const FHFL_CRC32: u64 = 0x4;

/// Extra record type of a file header: file-system redirection.
const FHEXTRA_REDIR: u64 = 0x5;

/// Host OS of the entries: Unix, whose attributes are a `st_mode`.
const HOST_UNIX: u64 = 1;
const REGULAR_FILE_MODE: u64 = 0o100_644;

/// The kinds of file-system redirection a RAR 5 entry can record.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Redirect {
    UnixSymlink = 1,
    WindowsSymlink = 2,
    Junction = 3,
    HardLink = 4,
    FileCopy = 5,
}

/// A RAR 5 archive under construction.
#[derive(Debug)]
pub(crate) struct Rar5 {
    bytes: Vec<u8>,
}

impl Rar5 {
    pub(crate) fn new() -> Self {
        let mut bytes = SIGNATURE.to_vec();
        let mut main = Vec::new();
        for field in [HEAD_MAIN, 0, 0] {
            vint(field, &mut main);
        }
        block(&mut bytes, &main, &[]);
        Self { bytes }
    }

    /// Adds a stored file.
    pub(crate) fn file(self, name: &str, data: &[u8]) -> Self {
        self.entry(name, data, &[])
    }

    /// Adds an entry redirecting to `target` instead of holding data.
    pub(crate) fn redirect(self, name: &str, kind: Redirect, target: &str) -> Self {
        let mut record = Vec::new();
        vint(FHEXTRA_REDIR, &mut record);
        vint(kind as u64, &mut record);
        vint(0, &mut record);
        vint(len(target.as_bytes()), &mut record);
        record.extend_from_slice(target.as_bytes());
        self.entry(name, &[], &extra_record(&record))
    }

    /// Ends the archive and writes it to `path`.
    pub(crate) fn write(mut self, path: &Path) {
        let mut end = Vec::new();
        for field in [HEAD_END, 0, 0] {
            vint(field, &mut end);
        }
        block(&mut self.bytes, &end, &[]);
        fs::write(path, self.bytes).expect("RAR fixture can be written");
    }

    fn entry(mut self, name: &str, data: &[u8], extra: &[u8]) -> Self {
        let mut flags = 0;
        if !extra.is_empty() {
            flags |= HFL_EXTRA;
        }
        if !data.is_empty() {
            flags |= HFL_DATA;
        }
        let mut header = Vec::new();
        vint(HEAD_FILE, &mut header);
        vint(flags, &mut header);
        if !extra.is_empty() {
            vint(len(extra), &mut header);
        }
        if !data.is_empty() {
            vint(len(data), &mut header);
        }
        vint(FHFL_CRC32, &mut header);
        vint(len(data), &mut header);
        vint(REGULAR_FILE_MODE, &mut header);
        header.extend_from_slice(&crc32(data).to_le_bytes());
        vint(0, &mut header);
        vint(HOST_UNIX, &mut header);
        vint(len(name.as_bytes()), &mut header);
        header.extend_from_slice(name.as_bytes());
        header.extend_from_slice(extra);
        block(&mut self.bytes, &header, data);
        self
    }
}

/// Prefixes an extra record (type and fields) with its size.
fn extra_record(record: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    vint(len(record), &mut out);
    out.extend_from_slice(record);
    out
}

/// Appends a block: CRC32 of the size and header, the header size, the header, then `data`.
fn block(out: &mut Vec<u8>, header: &[u8], data: &[u8]) {
    let mut sized = Vec::new();
    vint(len(header), &mut sized);
    sized.extend_from_slice(header);
    out.extend_from_slice(&crc32(&sized).to_le_bytes());
    out.extend_from_slice(&sized);
    out.extend_from_slice(data);
}

/// Appends `value` as a RAR 5 variable-length integer: 7 bits per byte, low bits first, the high
/// bit set on every byte but the last.
fn vint(mut value: u64, out: &mut Vec<u8>) {
    loop {
        let low = u8::try_from(value & 0x7f).expect("seven bits fit a byte");
        value >>= 7;
        if value == 0 {
            out.push(low);
            return;
        }
        out.push(low | 0x80);
    }
}

fn len(bytes: &[u8]) -> u64 {
    u64::try_from(bytes.len()).expect("fixture lengths fit a u64")
}

/// CRC-32 (IEEE 802.3, reflected polynomial `0xEDB88320`), as used by RAR headers and data.
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0_u32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}
