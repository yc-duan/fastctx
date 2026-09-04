//! Primitives behind the World's end-to-end encryption: HKDF sub-keys, XChaCha20-Poly1305
//! envelopes, HMAC tags, keyed hashes, and OS randomness.
//!
//! Every function is pure and every key is 32 bytes. Failures are returned, never masked:
//! an operating system that cannot produce randomness or a tag that does not verify is an
//! error the caller has to surface.

use chacha20poly1305::XChaCha20Poly1305;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use zeroize::{Zeroize, ZeroizeOnDrop};

/// Length of every symmetric key, MAC tag, and hash in the World.
pub(crate) const KEY_LEN: usize = 32;
/// XChaCha20-Poly1305 nonce length.
pub(crate) const NONCE_LEN: usize = 24;
/// Poly1305 tag appended to every ciphertext.
pub(crate) const TAG_LEN: usize = 16;

pub(crate) type Key32 = [u8; KEY_LEN];
pub(crate) type Nonce24 = [u8; NONCE_LEN];

/// The four sub-keys derived from one World key epoch (`design-transport.md` §2).
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub(crate) struct SubKeys {
    /// Control envelopes.
    pub(crate) msg: Key32,
    /// Bulk data blocks.
    pub(crate) blob: Key32,
    /// Keyed hashes for artifact and block ids.
    pub(crate) id: Key32,
    /// MACs over member records and grants.
    pub(crate) mac: Key32,
}

impl SubKeys {
    /// Derives the sub-keys of one World key.
    pub(crate) fn derive(world_key: &Key32) -> Self {
        Self {
            msg: hkdf_derive(world_key, b"msg"),
            blob: hkdf_derive(world_key, b"blob"),
            id: hkdf_derive(world_key, b"id"),
            mac: hkdf_derive(world_key, b"mac"),
        }
    }
}

impl std::fmt::Debug for SubKeys {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("SubKeys(..)")
    }
}

/// Fills a fixed-size array from the operating system's CSPRNG.
pub(crate) fn random_bytes<const N: usize>() -> Result<[u8; N], String> {
    let mut bytes = [0_u8; N];
    getrandom::fill(&mut bytes)
        .map_err(|error| format!("The operating system could not provide randomness: {error}"))?;
    Ok(bytes)
}

/// Plain SHA-256.
pub(crate) fn sha256(data: &[u8]) -> Key32 {
    Sha256::digest(data).into()
}

/// SHA-256 over several parts, equivalent to hashing their concatenation.
pub(crate) fn sha256_parts(parts: &[&[u8]]) -> Key32 {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part);
    }
    hasher.finalize().into()
}

/// HKDF-SHA256 with the World's domain prefix on the info string and no salt.
pub(crate) fn hkdf_derive(ikm: &[u8], label: &[u8]) -> Key32 {
    let mut info = Vec::with_capacity(super::SIGNATURE_DOMAIN_PREFIX.len() + label.len());
    info.extend_from_slice(super::SIGNATURE_DOMAIN_PREFIX.as_bytes());
    info.extend_from_slice(label);
    let mut output = [0_u8; KEY_LEN];
    Hkdf::<Sha256>::new(None, ikm)
        .expand(&info, &mut output)
        .expect("a 32-byte HKDF output never exceeds the expansion limit");
    output
}

/// HMAC-SHA256.
pub(crate) fn hmac_sha256(key: &[u8], data: &[u8]) -> Key32 {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(data);
    mac.finalize().into_bytes().into()
}

/// Constant-time HMAC-SHA256 verification.
pub(crate) fn hmac_verify(key: &[u8], data: &[u8], tag: &[u8]) -> bool {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(data);
    mac.verify_slice(tag).is_ok()
}

/// A ciphertext with the random nonce it was sealed under.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Sealed {
    pub(crate) nonce: Nonce24,
    pub(crate) ciphertext: Vec<u8>,
}

impl Sealed {
    /// `nonce ‖ ciphertext`, the shape stored and sent for single-blob payloads.
    pub(crate) fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(NONCE_LEN + self.ciphertext.len());
        bytes.extend_from_slice(&self.nonce);
        bytes.extend_from_slice(&self.ciphertext);
        bytes
    }

    /// Splits `nonce ‖ ciphertext`.
    pub(crate) fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() < NONCE_LEN + TAG_LEN {
            return Err(
                "The sealed payload is too short to contain a nonce and a tag.".to_string(),
            );
        }
        let mut nonce = [0_u8; NONCE_LEN];
        nonce.copy_from_slice(&bytes[..NONCE_LEN]);
        Ok(Self {
            nonce,
            ciphertext: bytes[NONCE_LEN..].to_vec(),
        })
    }
}

/// Seals `plaintext` under a fresh random nonce, binding `aad`.
pub(crate) fn seal(key: &Key32, aad: &[u8], plaintext: &[u8]) -> Result<Sealed, String> {
    let nonce = random_bytes::<NONCE_LEN>()?;
    let ciphertext = seal_with_nonce(key, &nonce, aad, plaintext)?;
    Ok(Sealed { nonce, ciphertext })
}

/// Seals under a caller-chosen nonce. The caller guarantees the nonce is never reused with
/// this key; envelopes draw theirs from the CSPRNG, which makes reuse a 192-bit accident.
pub(crate) fn seal_with_nonce(
    key: &Key32,
    nonce: &Nonce24,
    aad: &[u8],
    plaintext: &[u8],
) -> Result<Vec<u8>, String> {
    let cipher = XChaCha20Poly1305::new_from_slice(key)
        .map_err(|_| "The encryption key has the wrong length.".to_string())?;
    cipher
        .encrypt(
            &chacha20poly1305::XNonce::from(*nonce),
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .map_err(|_| "Encryption failed.".to_string())
}

/// Opens a ciphertext sealed with `seal` or `seal_with_nonce`; any change to the nonce, the
/// additional data, or the ciphertext fails.
pub(crate) fn open(
    key: &Key32,
    nonce: &Nonce24,
    aad: &[u8],
    ciphertext: &[u8],
) -> Result<Vec<u8>, String> {
    let cipher = XChaCha20Poly1305::new_from_slice(key)
        .map_err(|_| "The decryption key has the wrong length.".to_string())?;
    cipher
        .decrypt(
            &chacha20poly1305::XNonce::from(*nonce),
            Payload {
                msg: ciphertext,
                aad,
            },
        )
        .map_err(|_| "The ciphertext does not authenticate under this key.".to_string())
}

/// Standard base64 without padding, the encoding of every binary field on the wire.
pub(crate) fn b64_encode(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD_NO_PAD.encode(bytes)
}

pub(crate) fn b64_decode(text: &str) -> Result<Vec<u8>, String> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD_NO_PAD
        .decode(text)
        .map_err(|error| format!("Invalid base64: {error}"))
}

/// URL-safe base64 without padding, used where the value may travel in a URL or be pasted.
pub(crate) fn b64url_encode(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

pub(crate) fn b64url_decode(text: &str) -> Result<Vec<u8>, String> {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(text)
        .map_err(|error| format!("Invalid base64url: {error}"))
}

/// Decodes a hex string into a fixed-size array.
pub(crate) fn hex_array<const N: usize>(text: &str, label: &str) -> Result<[u8; N], String> {
    let bytes = hex::decode(text).map_err(|error| format!("Invalid {label}: {error}"))?;
    <[u8; N]>::try_from(bytes).map_err(|bytes| {
        format!(
            "Invalid {label}: expected {N} bytes, found {}.",
            bytes.len()
        )
    })
}

/// Decodes a base64 string into a fixed-size array.
pub(crate) fn b64_array<const N: usize>(text: &str, label: &str) -> Result<[u8; N], String> {
    let bytes = b64_decode(text).map_err(|error| format!("Invalid {label}: {error}"))?;
    <[u8; N]>::try_from(bytes).map_err(|bytes| {
        format!(
            "Invalid {label}: expected {N} bytes, found {}.",
            bytes.len()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::{Sealed, SubKeys, hkdf_derive, hmac_sha256, hmac_verify, open, random_bytes, seal};

    #[test]
    fn sealed_payloads_open_only_with_the_same_key_nonce_and_aad() {
        let key = random_bytes::<32>().unwrap();
        let sealed = seal(&key, b"header", b"secret body").unwrap();
        assert_eq!(
            open(&key, &sealed.nonce, b"header", &sealed.ciphertext).unwrap(),
            b"secret body"
        );
        assert!(open(&key, &sealed.nonce, b"headex", &sealed.ciphertext).is_err());
        let mut flipped = sealed.ciphertext.clone();
        flipped[0] ^= 1;
        assert!(open(&key, &sealed.nonce, b"header", &flipped).is_err());
        let other = random_bytes::<32>().unwrap();
        assert!(open(&other, &sealed.nonce, b"header", &sealed.ciphertext).is_err());
        let round_trip = Sealed::from_bytes(&sealed.to_bytes()).unwrap();
        assert_eq!(round_trip, sealed);
    }

    #[test]
    fn sub_keys_are_distinct_and_deterministic() {
        let key = [7_u8; 32];
        let first = SubKeys::derive(&key);
        let second = SubKeys::derive(&key);
        assert_eq!(first.msg, second.msg);
        assert_ne!(first.msg, first.blob);
        assert_ne!(first.id, first.mac);
        assert_ne!(hkdf_derive(&key, b"msg"), hkdf_derive(&key, b"msg2"));
    }

    #[test]
    fn hmac_tags_verify_and_reject_a_changed_byte() {
        let key = [1_u8; 32];
        let tag = hmac_sha256(&key, b"record");
        assert!(hmac_verify(&key, b"record", &tag));
        assert!(!hmac_verify(&key, b"recorD", &tag));
        assert!(!hmac_verify(&[2_u8; 32], b"record", &tag));
    }
}
