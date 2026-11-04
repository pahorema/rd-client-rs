use crate::bencode::{self, Node, Value};
use sha1::{Digest, Sha1};
use sha2::Sha256;
use thiserror::Error;

/// One payload file; path components are relative to the torrent's root name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TorrentEntry {
    /// Path components (the name itself for single-file torrents).
    pub path: Vec<String>,
    /// Payload length in bytes.
    pub length: u64,
}

/// Parsed metadata with hashes computed over the exact encoded info dictionary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TorrentFile {
    /// Display name from the info dictionary.
    pub name: String,
    /// SHA-1 info hash, when v1 metadata is present.
    pub info_hash_v1: Option<[u8; 20]>,
    /// SHA-256 info hash, when v2 metadata is present.
    pub info_hash_v2: Option<[u8; 32]>,
    /// Primary announce URL.
    pub announce: Option<String>,
    /// Ordered tracker tiers.
    pub announce_list: Vec<Vec<String>>,
    /// Optional publisher/source marker in the info dictionary.
    pub source: Option<String>,
    /// Ordered payload files.
    pub files: Vec<TorrentEntry>,
    /// Sum of payload lengths.
    pub total_size: u64,
}

/// Invalid torrent structure or malformed bencode.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum ParseTorrentError {
    /// Bencode decoding failed.
    #[error(transparent)]
    Bencode(#[from] bencode::DecodeError),
    /// A required metadata field is absent.
    #[error("missing torrent field '{0}'")]
    Missing(&'static str),
    /// A field has an invalid type, encoding, or value.
    #[error("invalid torrent field '{0}'")]
    Invalid(&'static str),
    /// Payload lengths cannot be represented by a u64.
    #[error("torrent total size exceeds u64")]
    SizeOverflow,
}

impl TorrentFile {
    /// Parses torrent metadata, retaining exact info hashes rather than re-encoding info.
    pub fn parse(bytes: &[u8]) -> Result<Self, ParseTorrentError> {
        let root = bencode::decode(bytes)?;
        dictionary(&root, "root")?;
        let info = root
            .get(b"info")
            .ok_or(ParseTorrentError::Missing("info"))?;
        dictionary(info, "info")?;
        let name = text(required(info, b"name", "name")?, "name")?;
        let v2 = match info.get(b"meta version").map(|n| &n.value) {
            None => false,
            Some(Value::Integer(2)) => true,
            Some(_) => return Err(ParseTorrentError::Invalid("meta version")),
        };
        let mut files = Vec::new();
        if v2 {
            read_tree(
                required(info, b"file tree", "file tree")?,
                &mut Vec::new(),
                &mut files,
            )?;
        } else if let Some(list) = info.get(b"files") {
            for file in list.list().ok_or(ParseTorrentError::Invalid("files"))? {
                dictionary(file, "files entry")?;
                let parts = required(file, b"path", "path")?
                    .list()
                    .ok_or(ParseTorrentError::Invalid("path"))?;
                if parts.is_empty() {
                    return Err(ParseTorrentError::Invalid("path"));
                }
                let path = parts
                    .iter()
                    .map(|p| text(p, "path"))
                    .collect::<Result<_, _>>()?;
                let length = length(file)?;
                files.push(TorrentEntry { path, length });
            }
        } else {
            files.push(TorrentEntry {
                path: vec![name.clone()],
                length: length(info)?,
            });
        }
        let total_size = files
            .iter()
            .try_fold(0u64, |sum, file| sum.checked_add(file.length))
            .ok_or(ParseTorrentError::SizeOverflow)?;
        let announce = optional_text(&root, b"announce", "announce")?;
        let mut announce_list = Vec::new();
        if let Some(tiers) = root.get(b"announce-list") {
            for tier in tiers
                .list()
                .ok_or(ParseTorrentError::Invalid("announce-list"))?
            {
                announce_list.push(
                    tier.list()
                        .ok_or(ParseTorrentError::Invalid("announce-list tier"))?
                        .iter()
                        .map(|s| text(s, "announce-list URL"))
                        .collect::<Result<_, _>>()?,
                );
            }
        }
        Ok(Self {
            name,
            info_hash_v1: (!v2 || info.get(b"pieces").is_some())
                .then(|| Sha1::digest(&bytes[info.span.clone()]).into()),
            info_hash_v2: v2.then(|| Sha256::digest(&bytes[info.span.clone()]).into()),
            announce,
            announce_list,
            source: optional_text(info, b"source", "source")?,
            files,
            total_size,
        })
    }

    /// Returns the lowercase v1 hash when present, otherwise the lowercase v2 hash.
    #[must_use]
    pub fn info_hash_hex(&self) -> String {
        let bytes = self
            .info_hash_v1
            .as_ref()
            .map(<[u8; 20]>::as_slice)
            .or_else(|| self.info_hash_v2.as_ref().map(<[u8; 32]>::as_slice));
        bytes.map_or_else(String::new, |b| data_encoding::HEXLOWER.encode(b))
    }
}

fn dictionary(node: &Node<'_>, field: &'static str) -> Result<(), ParseTorrentError> {
    if matches!(node.value, Value::Dictionary(_)) {
        Ok(())
    } else {
        Err(ParseTorrentError::Invalid(field))
    }
}
fn required<'node, 'src>(
    node: &'node Node<'src>,
    key: &[u8],
    field: &'static str,
) -> Result<&'node Node<'src>, ParseTorrentError> {
    node.get(key).ok_or(ParseTorrentError::Missing(field))
}
fn text(node: &Node<'_>, field: &'static str) -> Result<String, ParseTorrentError> {
    node.bytes()
        .and_then(|s| std::str::from_utf8(s).ok())
        .map(str::to_owned)
        .ok_or(ParseTorrentError::Invalid(field))
}
fn optional_text(
    node: &Node<'_>,
    key: &[u8],
    field: &'static str,
) -> Result<Option<String>, ParseTorrentError> {
    node.get(key).map(|n| text(n, field)).transpose()
}
fn length(node: &Node<'_>) -> Result<u64, ParseTorrentError> {
    match required(node, b"length", "length")?.value {
        Value::Integer(n) => u64::try_from(n).map_err(|_| ParseTorrentError::Invalid("length")),
        _ => Err(ParseTorrentError::Invalid("length")),
    }
}

fn read_tree(
    node: &Node<'_>,
    path: &mut Vec<String>,
    files: &mut Vec<TorrentEntry>,
) -> Result<(), ParseTorrentError> {
    let Value::Dictionary(entries) = &node.value else {
        return Err(ParseTorrentError::Invalid("file tree"));
    };
    if entries.is_empty() {
        return Err(ParseTorrentError::Invalid("empty file tree"));
    }
    for (key, value) in entries {
        if key.is_empty() {
            if path.is_empty() || entries.len() != 1 {
                return Err(ParseTorrentError::Invalid("file tree leaf"));
            }
            dictionary(value, "file tree leaf")?;
            files.push(TorrentEntry {
                path: path.clone(),
                length: length(value)?,
            });
        } else {
            let component = std::str::from_utf8(key)
                .map_err(|_| ParseTorrentError::Invalid("file tree path"))?;
            path.push(component.to_owned());
            read_tree(value, path, files)?;
            path.pop();
        }
    }
    Ok(())
}
