//! A package's signature, checked as the official updater checks it: by the
//! one minisign key compiled in, over the whole file read once, the version
//! the signature names, when it names one, being the release's.

use std::io::Read as _;
use std::path::Path;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use minisign_verify::{PublicKey, Signature};

/// Checks the file at `path` against `signature`, the base64 a manifest
/// carries, for the release `version`.
pub fn file(path: &Path, signature: &str, version: &str) -> Result<(), String> {
    let key = PublicKey::decode(&text(fastctx_types::update::PUBLIC_KEY)?)
        .map_err(|error| format!("the program's own key: {error}"))?;
    let signature =
        Signature::decode(&text(signature)?).map_err(|error| format!("the package's signature: {error}"))?;
    let mut verifier = key
        .verify_stream(&signature)
        .map_err(|error| format!("the package's signature is not one this key made: {error}"))?;
    let mut file = std::fs::File::open(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let mut buffer = vec![0; 256 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|error| format!("{}: {error}", path.display()))?;
        if read == 0 {
            break;
        }
        verifier.update(&buffer[..read]);
    }
    verifier.finalize().map_err(|_| "the package does not match its signature".to_string())?;
    // The trusted comment is signed too: a signature made for another
    // version is not taken for this one.
    let signed = signature.trusted_comment().split('\t').find_map(|field| field.strip_prefix("version:"));
    if let Some(signed) = signed
        && signed.trim_start_matches('v') != version
    {
        return Err(format!("the package was signed for {signed}, not {version}"));
    }
    Ok(())
}

fn text(base64: &str) -> Result<String, String> {
    let bytes = STANDARD.decode(base64.trim()).map_err(|error| format!("a signature in base64: {error}"))?;
    String::from_utf8(bytes).map_err(|_| "a signature that is not text".to_string())
}
