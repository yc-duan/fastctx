//! Member and hub identities: Ed25519 signing keys, X25519 wrap keys, fingerprints, and the
//! domain-separated signature scheme every World object uses.
//!
//! There are no certificates and no expiry. An identity is its public key; it dies only when
//! the World revokes it. Seeds are stored as small JSON files inside the owner-only World
//! directory.

use super::crypto::{Key32, hex_array, sha256};
use super::{SIGNATURE_DOMAIN_PREFIX, WorldPaths, read_optional, write_atomic};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::Path;
use x25519_dalek::{PublicKey as X25519Public, StaticSecret};
use zeroize::Zeroizing;

/// Length of an Ed25519 signature.
pub(crate) const SIGNATURE_LEN: usize = 64;
pub(crate) type Signature64 = [u8; SIGNATURE_LEN];

const KEY_FILE_VERSION: u32 = 1;

/// First 12 bytes of SHA-256 over a public key, shown as 24 hex characters.
#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub(crate) struct Fingerprint([u8; 12]);

impl Fingerprint {
    /// Fingerprint of an Ed25519 or X25519 public key.
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
    /// presented as another (`fastctx-world/v1/<domain>\0` is prefixed before signing).
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

/// A member's complete identity: the signing key plus the X25519 wrap key that receives
/// rotated World keys.
#[derive(Clone)]
pub(crate) struct Identity {
    signing: SigningIdentity,
    wrap: StaticSecret,
}

impl Identity {
    pub(crate) fn generate() -> Result<Self, String> {
        let wrap_seed = Zeroizing::new(super::crypto::random_bytes::<32>()?);
        Ok(Self {
            signing: SigningIdentity::generate()?,
            wrap: StaticSecret::from(*wrap_seed),
        })
    }

    pub(crate) fn signing(&self) -> &SigningIdentity {
        &self.signing
    }

    pub(crate) fn public_key(&self) -> Key32 {
        self.signing.public_key()
    }

    pub(crate) fn wrap_public(&self) -> Key32 {
        X25519Public::from(&self.wrap).to_bytes()
    }

    pub(crate) fn fingerprint(&self) -> Fingerprint {
        self.signing.fingerprint()
    }

    pub(crate) fn sign(&self, domain: &str, message: &[u8]) -> Signature64 {
        self.signing.sign(domain, message)
    }

    /// X25519 agreement between this member's wrap key and a peer's public key. A
    /// non-contributory result (a low-order peer point) is rejected rather than used.
    pub(crate) fn agree(&self, their_public: &Key32) -> Result<Key32, String> {
        agree(&self.wrap, their_public)
    }

    /// Loads both key files; `Ok(None)` when neither exists, an error when only one does.
    pub(crate) fn load(paths: &WorldPaths) -> Result<Option<Self>, String> {
        let signing = SigningIdentity::load(&paths.identity_key)?;
        let wrap = load_seed(&paths.wrap_key, "x25519")?;
        match (signing, wrap) {
            (Some(signing), Some(wrap)) => Ok(Some(Self {
                signing,
                wrap: StaticSecret::from(wrap),
            })),
            (None, None) => Ok(None),
            (Some(_), None) => Err(format!(
                "{} exists but {} is missing; the World identity is incomplete. Run 'fastctx node unenroll' and enroll again.",
                crate::paths::display_path(&paths.identity_key),
                crate::paths::display_path(&paths.wrap_key)
            )),
            (None, Some(_)) => Err(format!(
                "{} exists but {} is missing; the World identity is incomplete. Run 'fastctx node unenroll' and enroll again.",
                crate::paths::display_path(&paths.wrap_key),
                crate::paths::display_path(&paths.identity_key)
            )),
        }
    }

    pub(crate) fn save(&self, paths: &WorldPaths) -> Result<(), String> {
        self.signing.save(&paths.identity_key)?;
        save_seed(&paths.wrap_key, "x25519", self.wrap.as_bytes())
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

/// X25519 agreement with a one-shot secret; used by the key-rotation sender.
pub(crate) fn agree_ephemeral(their_public: &Key32) -> Result<(Key32, Key32), String> {
    let seed = Zeroizing::new(super::crypto::random_bytes::<32>()?);
    let secret = StaticSecret::from(*seed);
    let public = X25519Public::from(&secret).to_bytes();
    let shared = agree(&secret, their_public)?;
    Ok((public, shared))
}

fn agree(secret: &StaticSecret, their_public: &Key32) -> Result<Key32, String> {
    let shared = secret.diffie_hellman(&X25519Public::from(*their_public));
    if !shared.was_contributory() {
        return Err(
            "The peer's wrap key is a low-order point; refusing the agreement.".to_string(),
        );
    }
    Ok(shared.to_bytes())
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

#[cfg(test)]
mod tests {
    use super::{Fingerprint, Identity, SigningIdentity, agree_ephemeral, verify};

    #[test]
    fn signatures_bind_the_domain_and_the_message() {
        let identity = SigningIdentity::generate().unwrap();
        let signature = identity.sign("execute", b"payload");
        assert!(verify(&identity.public_key(), "execute", b"payload", &signature).is_ok());
        assert!(verify(&identity.public_key(), "call", b"payload", &signature).is_err());
        assert!(verify(&identity.public_key(), "execute", b"payloaD", &signature).is_err());
        let other = SigningIdentity::generate().unwrap();
        assert!(verify(&other.public_key(), "execute", b"payload", &signature).is_err());
    }

    #[test]
    fn identity_round_trips_through_its_key_files_and_agreements_match() {
        let temp = tempfile::tempdir().unwrap();
        let paths = super::super::WorldPaths::from_control(
            &crate::control::paths::ControlPaths::for_home(temp.path()),
        );
        paths.ensure().unwrap();
        assert!(Identity::load(&paths).unwrap().is_none());
        let identity = Identity::generate().unwrap();
        identity.save(&paths).unwrap();
        let loaded = Identity::load(&paths).unwrap().unwrap();
        assert_eq!(loaded.public_key(), identity.public_key());
        assert_eq!(loaded.wrap_public(), identity.wrap_public());
        assert_eq!(loaded.fingerprint().to_string().len(), 24);
        assert_eq!(
            Fingerprint::parse(&loaded.fingerprint().to_string()).unwrap(),
            identity.fingerprint()
        );

        let (ephemeral_public, shared_sender) = agree_ephemeral(&identity.wrap_public()).unwrap();
        let shared_receiver = identity.agree(&ephemeral_public).unwrap();
        assert_eq!(shared_sender, shared_receiver);
        assert!(identity.agree(&[0_u8; 32]).is_err());
    }
}
