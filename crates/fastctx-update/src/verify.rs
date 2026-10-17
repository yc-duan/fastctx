//! A package's signature, checked as the official updater checks it: by the
//! one minisign key compiled in, over the whole file read once. Every
//! release signs its packages bound to its version, so a signature that
//! names no version, or another, is refused: an older package is never
//! taken for a newer release, wherever its manifest came from.

use std::path::Path;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use minisign_verify::{PublicKey, Signature};

/// Checks the file at `path` against `signature`, the base64 a manifest
/// carries, for the release `version`.
pub fn file(path: &Path, signature: &str, version: &str) -> Result<(), String> {
    let file = std::fs::File::open(path).map_err(|error| format!("{}: {error}", path.display()))?;
    reader(file, signature, version).map_err(|error| format!("{}: {error}", path.display()))
}

/// Checks what `reader` gives, read once to its end, as `file` checks a file.
pub fn reader(mut reader: impl std::io::Read, signature: &str, version: &str) -> Result<(), String> {
    let (key, decoded) = decoded(signature)?;
    let mut verifier = key
        .verify_stream(&decoded)
        .map_err(|error| format!("the package's signature is not one this key made: {error}"))?;
    let mut buffer = vec![0; 256 * 1024];
    loop {
        let read = reader.read(&mut buffer).map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        verifier.update(&buffer[..read]);
    }
    verifier.finalize().map_err(|_| "the package does not match its signature".to_string())?;
    bound(&decoded, version)
}

/// Checks `bytes`, read once, against `signature` for the release `version`.
pub fn bytes(bytes: &[u8], signature: &str, version: &str) -> Result<(), String> {
    let (key, decoded) = decoded(signature)?;
    key.verify(bytes, &decoded, false).map_err(|_| "the file does not match its signature".to_string())?;
    bound(&decoded, version)
}

/// The release `signature` is bound to, as its trusted comment says. Only
/// a signature checked against what it signs says so for certain.
pub fn signed_version(signature: &str) -> Option<String> {
    let decoded = Signature::decode(&text(signature).ok()?).ok()?;
    signed(&decoded).map(|version| version.trim_start_matches('v').to_string())
}

fn decoded(signature: &str) -> Result<(PublicKey, Signature), String> {
    let key = PublicKey::decode(&text(fastctx_types::update::PUBLIC_KEY)?)
        .map_err(|error| format!("the program's own key: {error}"))?;
    let signature =
        Signature::decode(&text(signature)?).map_err(|error| format!("the package's signature: {error}"))?;
    Ok((key, signature))
}

fn signed(signature: &Signature) -> Option<&str> {
    signature.trusted_comment().split('\t').find_map(|field| field.strip_prefix("version:"))
}

/// The trusted comment is signed too: it names the release.
fn bound(signature: &Signature, version: &str) -> Result<(), String> {
    match signed(signature) {
        Some(signed) if signed.trim_start_matches('v') == version => Ok(()),
        Some(signed) => Err(format!("the package was signed for {signed}, not {version}")),
        None => Err(format!("the package's signature is bound to no version, so it is not taken for {version}")),
    }
}

fn text(base64: &str) -> Result<String, String> {
    let bytes = STANDARD.decode(base64.trim()).map_err(|error| format!("a signature in base64: {error}"))?;
    String::from_utf8(bytes).map_err(|_| "a signature that is not text".to_string())
}
