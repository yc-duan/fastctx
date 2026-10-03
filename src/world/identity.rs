//! Device and hub Ed25519 identities with domain-separated signatures.

use super::crypto::{Key32, hex_array, sha256};
use super::{SIGNATURE_DOMAIN_PREFIX, WorldPaths, read_optional, write_atomic};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::Path;
use zeroize::Zeroizing;

/// Length of an Ed25519 signature.
pub(crate) const SIGNATURE_LEN: usize = 64;
pub(crate) type Signature64 = [u8; SIGNATURE_LEN];

const KEY_FILE_VERSION: u32 = 1;

/// First 12 bytes of SHA-256 over a public key, shown as 24 hex characters.
#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub(crate) struct Fingerprint([u8; 12]);

impl Fingerprint {
    /// Fingerprint of an Ed25519 public key.
    pub(crate) fn of(public_key: &Key32) -> Self {
        let digest = sha256(public_key);
        let mut bytes = [0_u8; 12];
        bytes.copy_from_slice(&digest[..12]);
        Self(bytes)
    }

    pub(crate) fn parse(text: &str) -> Result<Self, String> {
        hex_array::<12>(text, "fingerprint").map(Self)
    }
}

impl fmt::Display for Fingerprint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&hex::encode(self.0))
    }
}

impl fmt::Debug for Fingerprint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "Fingerprint({self})")
    }
}

/// An Ed25519 key pair that signs World objects; the hub's identity is exactly this.
#[derive(Clone)]
pub(crate) struct SigningIdentity {
    signing: SigningKey,
}

impl SigningIdentity {
    pub(crate) fn generate() -> Result<Self, String> {
        let seed = Zeroizing::new(super::crypto::random_bytes::<32>()?);
        Ok(Self {
            signing: SigningKey::from_bytes(&seed),
        })
    }

    pub(crate) fn from_seed(seed: &Key32) -> Self {
        Self {
            signing: SigningKey::from_bytes(seed),
        }
    }

    pub(crate) fn public_key(&self) -> Key32 {
        self.signing.verifying_key().to_bytes()
    }

    pub(crate) fn fingerprint(&self) -> Fingerprint {
        Fingerprint::of(&self.public_key())
    }

    /// Signs `message` under a domain so a signature made for one object type can never be
    /// presented as another (`fastctx-world/v2/<domain>\0` is prefixed before signing).
    pub(crate) fn sign(&self, domain: &str, message: &[u8]) -> Signature64 {
        self.signing
            .sign(&signing_input(domain, message))
            .to_bytes()
    }

    pub(crate) fn load(path: &Path) -> Result<Option<Self>, String> {
        Ok(load_seed(path, "ed25519")?.map(|seed| Self::from_seed(&seed)))
    }

    pub(crate) fn save(&self, path: &Path) -> Result<(), String> {
        save_seed(path, "ed25519", self.signing.as_bytes())
    }
}

impl fmt::Debug for SigningIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "SigningIdentity({})", self.fingerprint())
    }
}

/// A member's signing identity.
#[derive(Clone)]
pub(crate) struct Identity {
    signing: SigningIdentity,
}

impl Identity {
    pub(crate) fn generate() -> Result<Self, String> {
        Ok(Self {
            signing: SigningIdentity::generate()?,
        })
    }

    pub(crate) fn signing(&self) -> &SigningIdentity {
        &self.signing
    }

    pub(crate) fn public_key(&self) -> Key32 {
        self.signing.public_key()
    }

    pub(crate) fn fingerprint(&self) -> Fingerprint {
        self.signing.fingerprint()
    }

    pub(crate) fn sign(&self, domain: &str, message: &[u8]) -> Signature64 {
        self.signing.sign(domain, message)
    }

    pub(crate) fn load(paths: &WorldPaths) -> Result<Option<Self>, String> {
        Ok(SigningIdentity::load(&paths.identity_key)?.map(|signing| Self { signing }))
    }
    pub(crate) fn save(&self, paths: &WorldPaths) -> Result<(), String> {
        self.signing.save(&paths.identity_key)
    }
}

impl fmt::Debug for Identity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "Identity({})", self.fingerprint())
    }
}

/// Verifies a domain-separated signature made by `SigningIdentity::sign`.
pub(crate) fn verify(
    public_key: &Key32,
    domain: &str,
    message: &[u8],
    signature: &[u8],
) -> Result<(), String> {
    let verifying = VerifyingKey::from_bytes(public_key)
        .map_err(|_| "The public key is not a valid Ed25519 key.".to_string())?;
    let signature = <[u8; SIGNATURE_LEN]>::try_from(signature)
        .map_err(|_| "The signature has the wrong length.".to_string())?;
    verifying
        .verify_strict(
            &signing_input(domain, message),
            &Signature::from_bytes(&signature),
        )
        .map_err(|_| format!("The {domain} signature does not verify."))
}

fn signing_input(domain: &str, message: &[u8]) -> Vec<u8> {
    let mut input =
        Vec::with_capacity(SIGNATURE_DOMAIN_PREFIX.len() + domain.len() + 1 + message.len());
    input.extend_from_slice(SIGNATURE_DOMAIN_PREFIX.as_bytes());
    input.extend_from_slice(domain.as_bytes());
    input.push(0);
    input.extend_from_slice(message);
    input
}

#[derive(Deserialize, Serialize)]
struct KeyFile {
    v: u32,
    kind: String,
    seed: String,
}

fn load_seed(path: &Path, kind: &str) -> Result<Option<Key32>, String> {
    let Some(bytes) = read_optional(path)? else {
        return Ok(None);
    };
    let file: KeyFile = serde_json::from_slice(&bytes).map_err(|error| {
        format!(
            "Cannot parse the key file {}: {error}",
            crate::paths::display_path(path)
        )
    })?;
    if file.v > KEY_FILE_VERSION {
        return Err(format!(
            "{} was written by a newer fastctx (format {}); this build reads format {} at most.",
            crate::paths::display_path(path),
            file.v,
            KEY_FILE_VERSION
        ));
    }
    if file.kind != kind {
        return Err(format!(
            "{} holds a {} key where a {kind} key was expected.",
            crate::paths::display_path(path),
            file.kind
        ));
    }
    let seed = hex_array::<32>(&file.seed, "key seed")
        .map_err(|error| format!("{}: {error}", crate::paths::display_path(path)))?;
    Ok(Some(seed))
}

fn save_seed(path: &Path, kind: &str, seed: &Key32) -> Result<(), String> {
    let file = KeyFile {
        v: KEY_FILE_VERSION,
        kind: kind.to_string(),
        seed: hex::encode(seed),
    };
    let json = serde_json::to_vec_pretty(&file)
        .map_err(|error| format!("Cannot encode the key file: {error}"))?;
    write_atomic(path, &json)
}
