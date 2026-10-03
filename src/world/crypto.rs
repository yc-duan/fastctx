//! Randomness, hashes, and wire encodings.
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
pub(crate) type Key32 = [u8; 32];
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

/// HMAC-SHA256.
pub(crate) fn hmac_sha256(key: &[u8], data: &[u8]) -> Key32 {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(data);
    mac.finalize().into_bytes().into()
}

/// Constant-time verification of the hub's bootstrap proof.
pub(crate) fn hmac_verify(key: &[u8], data: &[u8], tag: &[u8]) -> bool {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(data);
    mac.verify_slice(tag).is_ok()
}

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
