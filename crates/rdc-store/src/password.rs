//! Password hashes in the stored format.
//!
//! A hash is the standard base64 of a versioned binary record:
//!
//! * version 3: `0x01`, then big-endian `u32` PRF (0 HMAC-SHA1, 1
//!   HMAC-SHA256, 2 HMAC-SHA512), iteration count and salt length, the salt,
//!   and the PBKDF2 subkey filling the rest;
//! * version 2: `0x00`, a 16 byte salt and a 32 byte PBKDF2-HMAC-SHA1
//!   subkey derived with 1000 iterations.
//!
//! New hashes use version 3 with HMAC-SHA512, 100 000 iterations, a 16 byte
//! salt and a 32 byte subkey; verifying any weaker hash asks for a rehash.

use data_encoding::BASE64;
use pbkdf2::pbkdf2_hmac;
use sha1::Sha1;
use sha2::{Sha256, Sha512};
use subtle::ConstantTimeEq;

/// Iterations of new hashes; weaker stored hashes are upgraded.
const ITERATIONS: u32 = 100_000;
const SALT_LENGTH: usize = 16;
const SUBKEY_LENGTH: usize = 32;
/// Shortest salt and subkey accepted in a version 3 hash.
const MINIMUM_LENGTH: usize = 16;
/// Length of the version 3 header: marker, PRF, iterations, salt length.
const HEADER_LENGTH: usize = 13;
const V2_ITERATIONS: u32 = 1000;
const V2_LENGTH: usize = 1 + SALT_LENGTH + SUBKEY_LENGTH;

const PRF_SHA1: u32 = 0;
const PRF_SHA256: u32 = 1;
const PRF_SHA512: u32 = 2;

/// Outcome of [`verify`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Verification {
    /// The password does not match (or the hash is unreadable).
    Failed,
    /// The password matches.
    Success,
    /// The password matches a hash weaker than new hashes; store a new one.
    SuccessRehashNeeded,
}

/// A new version 3 hash of `password` with a random salt.
pub(crate) fn hash(password: &str) -> String {
    let mut salt = [0; SALT_LENGTH];
    getrandom::fill(&mut salt).expect("the operating system provides random bytes");
    hash_with_salt(password, &salt)
}

fn hash_with_salt(password: &str, salt: &[u8; SALT_LENGTH]) -> String {
    let mut record = Vec::with_capacity(HEADER_LENGTH + SALT_LENGTH + SUBKEY_LENGTH);
    record.push(0x01);
    record.extend_from_slice(&PRF_SHA512.to_be_bytes());
    record.extend_from_slice(&ITERATIONS.to_be_bytes());
    record.extend_from_slice(
        &u32::try_from(SALT_LENGTH)
            .expect("small salt")
            .to_be_bytes(),
    );
    record.extend_from_slice(salt);
    let mut subkey = [0; SUBKEY_LENGTH];
    pbkdf2_hmac::<Sha512>(password.as_bytes(), salt, ITERATIONS, &mut subkey);
    record.extend_from_slice(&subkey);
    BASE64.encode(&record)
}

/// Checks `password` against a stored hash, in time independent of where
/// the subkeys differ.
pub(crate) fn verify(hashed: &str, password: &str) -> Verification {
    let Ok(record) = BASE64.decode(hashed.trim().as_bytes()) else {
        return Verification::Failed;
    };
    match record.first() {
        Some(0x00) if verify_v2(&record, password) => Verification::SuccessRehashNeeded,
        Some(0x01) => match verify_v3(&record, password) {
            Some((PRF_SHA512, iterations)) if iterations >= ITERATIONS => Verification::Success,
            Some(_) => Verification::SuccessRehashNeeded,
            None => Verification::Failed,
        },
        _ => Verification::Failed,
    }
}

fn verify_v2(record: &[u8], password: &str) -> bool {
    if record.len() != V2_LENGTH {
        return false;
    }
    let (salt, expected) = record[1..].split_at(SALT_LENGTH);
    let mut subkey = [0; SUBKEY_LENGTH];
    pbkdf2_hmac::<Sha1>(password.as_bytes(), salt, V2_ITERATIONS, &mut subkey);
    bool::from(subkey.ct_eq(expected))
}

/// The PRF and iteration count of a matching version 3 hash.
fn verify_v3(record: &[u8], password: &str) -> Option<(u32, u32)> {
    let header = |offset: usize| -> Option<u32> {
        let bytes = record.get(offset..offset + 4)?;
        Some(u32::from_be_bytes(bytes.try_into().ok()?))
    };
    let prf = header(1)?;
    let iterations = header(5)?;
    let salt_length = usize::try_from(header(9)?).ok()?;
    // Iteration counts are signed 31-bit values in the stored format.
    if iterations == 0 || i32::try_from(iterations).is_err() || salt_length < MINIMUM_LENGTH {
        return None;
    }
    let salt = record.get(HEADER_LENGTH..HEADER_LENGTH.checked_add(salt_length)?)?;
    let expected = &record[HEADER_LENGTH + salt_length..];
    if expected.len() < MINIMUM_LENGTH {
        return None;
    }
    let mut subkey = vec![0; expected.len()];
    match prf {
        PRF_SHA1 => pbkdf2_hmac::<Sha1>(password.as_bytes(), salt, iterations, &mut subkey),
        PRF_SHA256 => pbkdf2_hmac::<Sha256>(password.as_bytes(), salt, iterations, &mut subkey),
        PRF_SHA512 => pbkdf2_hmac::<Sha512>(password.as_bytes(), salt, iterations, &mut subkey),
        _ => return None,
    }
    bool::from(subkey.ct_eq(expected)).then_some((prf, iterations))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Version 3, HMAC-SHA512, 100 000 iterations, salt `00..0f`, password
    /// `Passw0rd!` (computed independently with Python's `hashlib`).
    const V3_SHA512: &str =
        "AQAAAAIAAYagAAAAEAABAgMEBQYHCAkKCwwNDg9tHDBVNhTVA+6E1t6Gpb7bZypsIX6UNUiGvKx4ddfq6Q==";
    /// Version 3, HMAC-SHA256, 10 000 iterations, salt `00..0f`.
    const V3_SHA256: &str =
        "AQAAAAEAACcQAAAAEAABAgMEBQYHCAkKCwwNDg8a3RJST3ZL0eWWP7ZG5qnxmnu+1xsChmurHg4P2C3T8g==";
    /// Version 3, HMAC-SHA1, 100 000 iterations, salt `00..0f`.
    const V3_SHA1: &str =
        "AQAAAAAAAYagAAAAEAABAgMEBQYHCAkKCwwNDg+GRzFOK2T0J7xTMbGlYzYFr8bnSUiu8kSMSXRDlY5JHg==";
    /// Version 3, HMAC-SHA512, 200 000 iterations, 24 byte salt, 48 byte subkey.
    const V3_LONG: &str = "AQAAAAIAAw1AAAAAGAABAgMEBQYHCAkKCwwNDg8QERITFBUWF9ZLESyuuanLcKb4RLeglBvJNrh6X86LISis7Ku2IkJuQN/QJ2VkPAuVirBY/G4m8Q==";
    /// Version 2, salt `10..1f`.
    const V2: &str = "ABAREhMUFRYXGBkaGxwdHh8D0JclqrQbeYk+aqPTC8X10EUr7PH3FRUb6YwVnRmreg==";

    #[test]
    fn new_hashes_use_the_current_format() {
        let salt: [u8; SALT_LENGTH] =
            core::array::from_fn(|index| u8::try_from(index).expect("small"));
        assert_eq!(hash_with_salt("Passw0rd!", &salt), V3_SHA512);
    }

    #[test]
    fn new_hashes_are_salted_and_verify() {
        let first = hash("Passw0rd!");
        assert_ne!(first, hash("Passw0rd!"));
        assert_eq!(verify(&first, "Passw0rd!"), Verification::Success);
        assert_eq!(verify(&first, "passw0rd!"), Verification::Failed);
    }

    #[test]
    fn current_hashes_verify_without_rehash() {
        assert_eq!(verify(V3_SHA512, "Passw0rd!"), Verification::Success);
        assert_eq!(verify(V3_LONG, "Passw0rd!"), Verification::Success);
        assert_eq!(verify(V3_SHA512, "Passw0rd?"), Verification::Failed);
    }

    #[test]
    fn weaker_hashes_verify_and_ask_for_a_rehash() {
        for hashed in [V3_SHA256, V3_SHA1, V2] {
            assert_eq!(
                verify(hashed, "Passw0rd!"),
                Verification::SuccessRehashNeeded,
                "{hashed}"
            );
            assert_eq!(verify(hashed, "wrong1"), Verification::Failed, "{hashed}");
        }
    }

    #[test]
    fn unreadable_hashes_fail() {
        let decode = |hashed: &str| BASE64.decode(hashed.as_bytes()).expect("base64");
        let mut short_subkey = decode(V3_SHA512);
        short_subkey.truncate(HEADER_LENGTH + SALT_LENGTH + 15);
        let mut unknown_prf = decode(V3_SHA512);
        unknown_prf[4] = 3;
        let mut short_salt = decode(V3_SHA512);
        short_salt[12] = 15;
        let mut long_v2 = decode(V2);
        long_v2.push(0);
        for record in [
            short_subkey,
            unknown_prf,
            short_salt,
            long_v2,
            vec![0x02; 49],
            vec![0x01],
        ] {
            let hashed = BASE64.encode(&record);
            assert_eq!(
                verify(&hashed, "Passw0rd!"),
                Verification::Failed,
                "{hashed}"
            );
        }
        for hashed in ["", "not base64!", "AQ=="] {
            assert_eq!(
                verify(hashed, "Passw0rd!"),
                Verification::Failed,
                "{hashed:?}"
            );
        }
    }
}
