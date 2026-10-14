//! Device keys and signatures.
//!
//! Every machine signs what it says with its own Ed25519 key. Every
//! signature is made under a domain that names what is signed, so a
//! signature made for one kind of statement never verifies as another: the
//! signed bytes are `fastctx-world/3/<domain>\0` followed by the statement.

use std::fmt;
use std::io;
use std::path::Path;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ed25519_dalek::{Signer as _, SigningKey, VerifyingKey};
use fastctx_platform::{Permissions, publish_new};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use zeroize::Zeroizing;

const PREFIX: &str = "fastctx-world/3/";

/// What a signature vouches for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Domain {
    /// A machine proving to the hub that it holds its key, bound to one TLS
    /// connection.
    Session,
    /// A machine handing over what happens in a session it holds, to a
    /// machine watching it.
    Event,
    /// Metadata and byte-block digests belonging to one user's file read.
    FileRead,
    /// An entry of a project's shared ledger, as its author wrote it.
    Ledger,
    /// A machine proving to another, on a direct link between them, that it
    /// holds its key, bound to one TLS connection.
    Peer,
    /// An old hub proving to the place its hub moves to that it holds the
    /// move's key, bound to one TLS connection.
    Handover,
}

impl Domain {
    fn name(self) -> &'static str {
        match self {
            Domain::Session => "session",
            Domain::Event => "event",
            Domain::FileRead => "file-read",
            Domain::Ledger => "ledger",
            Domain::Peer => "peer",
            Domain::Handover => "handover",
        }
    }
}

fn signing_input(domain: Domain, bytes: &[u8]) -> Vec<u8> {
    let name = domain.name();
    let mut input = Vec::with_capacity(PREFIX.len() + name.len() + 1 + bytes.len());
    input.extend_from_slice(PREFIX.as_bytes());
    input.extend_from_slice(name.as_bytes());
    input.push(0);
    input.extend_from_slice(bytes);
    input
}

/// A machine's private key, or a token's one-time key.
pub struct SecretKey {
    key: SigningKey,
}

impl SecretKey {
    pub fn generate() -> io::Result<Self> {
        let seed = Zeroizing::new(random::<32>()?);
        Ok(Self::from_seed(&seed))
    }

    pub fn from_seed(seed: &[u8; 32]) -> Self {
        Self { key: SigningKey::from_bytes(seed) }
    }

    pub fn seed(&self) -> Zeroizing<[u8; 32]> {
        Zeroizing::new(self.key.to_bytes())
    }

    pub fn public(&self) -> PublicKey {
        PublicKey(self.key.verifying_key().to_bytes())
    }

    pub fn sign(&self, domain: Domain, bytes: &[u8]) -> Signature {
        Signature(self.key.sign(&signing_input(domain, bytes)).to_bytes())
    }

    /// Reads a key written by `create`.
    pub fn load(path: &Path) -> io::Result<Self> {
        let bytes = Zeroizing::new(std::fs::read(path)?);
        let file: KeyFile = serde_json::from_slice(&bytes)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, format!("{}: {error}", path.display())))?;
        if file.kind != "ed25519" {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{} holds a {} key, not an ed25519 key", path.display(), file.kind),
            ));
        }
        let seed = Zeroizing::new(decode_array::<32>(&file.seed).map_err(|error| {
            io::Error::new(io::ErrorKind::InvalidData, format!("{}: the seed {error}", path.display()))
        })?);
        Ok(Self::from_seed(&seed))
    }

    /// Writes this key to `path`, readable by its owner alone. A key once
    /// written is never replaced: whatever trusts it would stop trusting
    /// this machine.
    pub fn create(&self, path: &Path) -> io::Result<()> {
        let file = KeyFile { kind: "ed25519".to_string(), seed: Zeroizing::new(URL_SAFE_NO_PAD.encode(*self.seed())) };
        let bytes = Zeroizing::new(serde_json::to_vec(&file).map_err(io::Error::other)?);
        publish_new(path, &bytes, Permissions::Private)
    }
}

impl fmt::Debug for SecretKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SecretKey({})", self.public())
    }
}

#[derive(Serialize, Deserialize)]
struct KeyFile {
    kind: String,
    seed: Zeroizing<String>,
}

/// A public key: who signed.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PublicKey([u8; 32]);

impl PublicKey {
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Whether `signature` is this key's signature of `bytes` in `domain`.
    pub fn verify(&self, domain: Domain, bytes: &[u8], signature: &Signature) -> Result<(), BadSignature> {
        let key = VerifyingKey::from_bytes(&self.0).map_err(|_| BadSignature)?;
        let signature = ed25519_dalek::Signature::from_bytes(&signature.0);
        // Strict verification refuses the malleable and small-order forms a
        // lenient verifier lets through.
        key.verify_strict(&signing_input(domain, bytes), &signature).map_err(|_| BadSignature)
    }
}

impl fmt::Display for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&URL_SAFE_NO_PAD.encode(self.0))
    }
}

impl std::str::FromStr for PublicKey {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, String> {
        decode_array::<32>(text).map(PublicKey).map_err(|error| format!("the key {error}"))
    }
}

impl fmt::Debug for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PublicKey({self})")
    }
}

impl Serialize for PublicKey {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for PublicKey {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        decode_array::<32>(&text).map(PublicKey).map_err(serde::de::Error::custom)
    }
}

/// An Ed25519 signature.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Signature([u8; 64]);

impl Signature {
    pub fn to_bytes(&self) -> [u8; 64] {
        self.0
    }

    pub fn from_bytes(bytes: [u8; 64]) -> Self {
        Self(bytes)
    }
}

impl fmt::Debug for Signature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Signature({})", URL_SAFE_NO_PAD.encode(self.0))
    }
}

impl Serialize for Signature {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&URL_SAFE_NO_PAD.encode(self.0))
    }
}

impl<'de> Deserialize<'de> for Signature {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        decode_array::<64>(&text).map(Signature).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("the signature does not verify")]
pub struct BadSignature;

/// A statement and its signature. The statement is kept as the exact bytes
/// that were signed, so it verifies the same however the JSON around it is
/// rewritten on the way.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Signed {
    pub data: Bytes,
    pub sig: Signature,
}

/// Why a signed statement cannot be taken.
#[derive(Debug, thiserror::Error)]
pub enum Invalid {
    #[error("the signature does not verify")]
    Signature,
    #[error("the statement cannot be read: {0}")]
    Unreadable(serde_json::Error),
}

impl Signed {
    pub fn new(key: &SecretKey, domain: Domain, statement: &impl Serialize) -> Self {
        let data = serde_json::to_vec(statement).expect("the World's statements always serialize");
        let sig = key.sign(domain, &data);
        Self { data: Bytes(data), sig }
    }

    /// The statement, read without checking who signed it: for finding out
    /// who claims to have signed it, before checking that they did.
    pub fn claim<T: DeserializeOwned>(&self) -> Result<T, Invalid> {
        serde_json::from_slice(&self.data.0).map_err(Invalid::Unreadable)
    }

    /// The statement, if `key` signed it in `domain`.
    pub fn open<T: DeserializeOwned>(&self, key: &PublicKey, domain: Domain) -> Result<T, Invalid> {
        key.verify(domain, &self.data.0, &self.sig).map_err(|BadSignature| Invalid::Signature)?;
        self.claim()
    }

    /// Signs `bytes` as they are, for a statement in a form other than
    /// plain JSON, such as a packed history (`crate::history::pack`).
    pub fn of_bytes(key: &SecretKey, domain: Domain, bytes: Vec<u8>) -> Self {
        let sig = key.sign(domain, &bytes);
        Self { data: Bytes(bytes), sig }
    }

    /// The signed bytes, if `key` signed them in `domain`.
    pub fn verified(&self, key: &PublicKey, domain: Domain) -> Result<&[u8], Invalid> {
        key.verify(domain, &self.data.0, &self.sig).map_err(|BadSignature| Invalid::Signature)?;
        Ok(&self.data.0)
    }
}

/// Bytes carried in JSON as unpadded URL-safe base64.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Bytes(pub Vec<u8>);

impl fmt::Debug for Bytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Bytes({} bytes)", self.0.len())
    }
}

impl Serialize for Bytes {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&URL_SAFE_NO_PAD.encode(&self.0))
    }
}

impl<'de> Deserialize<'de> for Bytes {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        URL_SAFE_NO_PAD.decode(text.as_bytes()).map(Bytes).map_err(serde::de::Error::custom)
    }
}

/// A secret of 32 random bytes, such as an invitation's key seed or the
/// hub's founding secret.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(pub Zeroizing<[u8; 32]>);

impl Secret {
    pub fn generate() -> io::Result<Self> {
        Ok(Self(Zeroizing::new(random::<32>()?)))
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(..)")
    }
}

impl Serialize for Secret {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&URL_SAFE_NO_PAD.encode(*self.0))
    }
}

impl<'de> Deserialize<'de> for Secret {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = Zeroizing::new(String::deserialize(deserializer)?);
        decode_array::<32>(&text).map(|bytes| Secret(Zeroizing::new(bytes))).map_err(serde::de::Error::custom)
    }
}

/// A SHA-256 digest, carried as base64url.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Digest(pub [u8; 32]);

impl fmt::Display for Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&URL_SAFE_NO_PAD.encode(self.0))
    }
}

impl fmt::Debug for Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Digest({self})")
    }
}

impl Serialize for Digest {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Digest {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        decode_array::<32>(&text).map(Digest).map_err(serde::de::Error::custom)
    }
}

/// SHA-256 of `bytes`.
pub fn sha256(bytes: &[u8]) -> [u8; 32] {
    use sha2::Digest as _;
    sha2::Sha256::digest(bytes).into()
}

/// `N` random bytes from the operating system.
pub fn random<const N: usize>() -> io::Result<[u8; N]> {
    let mut bytes = [0_u8; N];
    getrandom::fill(&mut bytes).map_err(io::Error::other)?;
    Ok(bytes)
}

pub(crate) fn decode_array<const N: usize>(text: &str) -> Result<[u8; N], String> {
    let bytes = URL_SAFE_NO_PAD.decode(text.as_bytes()).map_err(|error| format!("is not base64url: {error}"))?;
    <[u8; N]>::try_from(bytes.as_slice()).map_err(|_| format!("is {} bytes, not {N}", bytes.len()))
}
