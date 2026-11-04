use thiserror::Error;
use url::Url;

/// Decoded magnet metadata; unknown query parameters retain their order and values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MagnetLink {
    /// SHA-1 hash from a hexadecimal or base32 btih topic.
    pub info_hash_v1: Option<[u8; 20]>,
    /// SHA-256 hash from a btmh SHA-256 multihash topic.
    pub info_hash_v2: Option<[u8; 32]>,
    /// Percent-decoded display name, absent when no dn parameter exists.
    pub display_name: Option<String>,
    /// Percent-decoded trackers in query order, including duplicates.
    pub trackers: Vec<String>,
    /// Unknown parameters, including unsupported exact-topic types.
    pub other_params: Vec<(String, String)>,
}

/// Invalid magnet URL or missing/malformed supported info hash.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum ParseMagnetError {
    /// The input is not an absolute magnet URI.
    #[error("invalid magnet URI")]
    InvalidUri,
    /// No supported btih or btmh exact topic exists.
    #[error("magnet URI has no supported info hash")]
    MissingHash,
    /// An exact topic has the wrong length, alphabet, or multihash prefix.
    #[error("invalid magnet info hash '{0}'")]
    InvalidHash(String),
    /// Multiple exact topics disagree for the same hash version.
    #[error("conflicting magnet info hashes")]
    ConflictingHashes,
}

impl MagnetLink {
    /// Parses v1 hex/base32 topics and v2 SHA-256 multihash topics.
    pub fn parse(input: &str) -> Result<Self, ParseMagnetError> {
        let url = Url::parse(input).map_err(|_| ParseMagnetError::InvalidUri)?;
        if url.scheme() != "magnet" {
            return Err(ParseMagnetError::InvalidUri);
        }
        let mut result = Self {
            info_hash_v1: None,
            info_hash_v2: None,
            display_name: None,
            trackers: Vec::new(),
            other_params: Vec::new(),
        };
        for (key, value) in url.query_pairs() {
            if key.eq_ignore_ascii_case("xt") {
                let lower = value.to_ascii_lowercase();
                if let Some(hash) = lower.strip_prefix("urn:btih:") {
                    let bytes = match hash.len() {
                        40 => data_encoding::HEXLOWER.decode(hash.as_bytes()),
                        32 => {
                            data_encoding::BASE32_NOPAD.decode(hash.to_ascii_uppercase().as_bytes())
                        }
                        _ => return Err(ParseMagnetError::InvalidHash(value.into_owned())),
                    }
                    .map_err(|_| ParseMagnetError::InvalidHash(value.to_string()))?;
                    let hash = bytes
                        .try_into()
                        .map_err(|_| ParseMagnetError::InvalidHash(value.to_string()))?;
                    set_hash(&mut result.info_hash_v1, hash)?;
                } else if let Some(hash) = lower.strip_prefix("urn:btmh:") {
                    let hash = hash
                        .strip_prefix("1220")
                        .filter(|h| h.len() == 64)
                        .ok_or_else(|| ParseMagnetError::InvalidHash(value.to_string()))?;
                    let bytes = data_encoding::HEXLOWER
                        .decode(hash.as_bytes())
                        .map_err(|_| ParseMagnetError::InvalidHash(value.to_string()))?;
                    let hash = bytes
                        .try_into()
                        .map_err(|_| ParseMagnetError::InvalidHash(value.to_string()))?;
                    set_hash(&mut result.info_hash_v2, hash)?;
                } else {
                    result
                        .other_params
                        .push((key.into_owned(), value.into_owned()));
                }
            } else if key.eq_ignore_ascii_case("dn") {
                if result.display_name.is_none() {
                    result.display_name = Some(value.into_owned());
                }
            } else if key.eq_ignore_ascii_case("tr") {
                result.trackers.push(value.into_owned());
            } else {
                result
                    .other_params
                    .push((key.into_owned(), value.into_owned()));
            }
        }
        if result.info_hash_v1.is_none() && result.info_hash_v2.is_none() {
            return Err(ParseMagnetError::MissingHash);
        }
        Ok(result)
    }

    /// Returns lowercase hexadecimal, preferring v1 when both versions exist.
    #[must_use]
    pub fn info_hash_hex(&self) -> String {
        self.info_hash_v1
            .as_ref()
            .map(<[u8; 20]>::as_slice)
            .or_else(|| self.info_hash_v2.as_ref().map(<[u8; 32]>::as_slice))
            .map_or_else(String::new, |hash| data_encoding::HEXLOWER.encode(hash))
    }
}
fn set_hash<const N: usize>(
    slot: &mut Option<[u8; N]>,
    hash: [u8; N],
) -> Result<(), ParseMagnetError> {
    if slot.is_some_and(|previous| previous != hash) {
        return Err(ParseMagnetError::ConflictingHashes);
    }
    *slot = Some(hash);
    Ok(())
}
