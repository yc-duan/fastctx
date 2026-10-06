//! Sealing for one member: its X25519 key, and RFC 9180 HPKE to it.
//!
//! Every member holds, beside the Ed25519 key it signs with, an X25519 key
//! that others encrypt to. The two are separate keys for separate uses;
//! neither is derived from the other. The public half is in the member's
//! self-signed record, so whoever checks the members table knows it, and
//! nobody can put another in its place.
//!
//! What waits for a member that may not be there, a letter or a ledger
//! entry's key, is sealed to this key with HPKE in base mode:
//! DHKEM(X25519, HKDF-SHA256), HKDF-SHA256 and ChaCha20-Poly1305. Who
//! sealed it is said by the signature of the envelope it travels in, so the
//! sender is not authenticated by HPKE itself. The same 32 secret bytes are
//! the member's static key in its Noise sessions (`crate::noise`).

use std::fmt;
use std::io;
use std::path::Path;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use fastctx_platform::{Permissions, publish_new};
use hpke::aead::ChaCha20Poly1305;
use hpke::kdf::HkdfSha256;
use hpke::kem::X25519HkdfSha256;
use hpke::{Deserializable as _, Kem as _, OpModeR, OpModeS, Serializable as _};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use zeroize::Zeroizing;

use crate::sign::{Bytes, decode_array, random};

type Kem = X25519HkdfSha256;

/// A member's X25519 private key.
pub struct SealKey {
    secret: <Kem as hpke::Kem>::PrivateKey,
    seed: Zeroizing<[u8; 32]>,
    public: SealPublic,
}

impl SealKey {
    pub fn generate() -> io::Result<Self> {
        let seed = Zeroizing::new(random::<32>()?);
        Ok(Self::from_seed(&seed))
    }

    /// Any 32 bytes are a key: X25519 clamps them where it uses them.
    pub fn from_seed(seed: &[u8; 32]) -> Self {
        let secret = <Kem as hpke::Kem>::PrivateKey::from_bytes(seed).expect("an X25519 key is any 32 bytes");
        let public = SealPublic(Kem::sk_to_pk(&secret).to_bytes().into());
        Self { secret, seed: Zeroizing::new(*seed), public }
    }

    /// The secret bytes, for the Noise sessions this member takes part in.
    pub fn seed(&self) -> &Zeroizing<[u8; 32]> {
        &self.seed
    }

    pub fn public(&self) -> SealPublic {
        self.public
    }

    /// Reads a key written by `create`.
    pub fn load(path: &Path) -> io::Result<Self> {
        let bytes = Zeroizing::new(std::fs::read(path)?);
        let file: KeyFile = serde_json::from_slice(&bytes)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, format!("{}: {error}", path.display())))?;
        if file.kind != "x25519" {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{} holds a {} key, not an x25519 key", path.display(), file.kind),
            ));
        }
        let seed = Zeroizing::new(decode_array::<32>(&file.seed).map_err(|error| {
            io::Error::new(io::ErrorKind::InvalidData, format!("{}: the seed {error}", path.display()))
        })?);
        Ok(Self::from_seed(&seed))
    }

    /// Writes this key to `path`, readable by its owner alone. A key once
    /// written is never replaced: what was sealed to it would be lost.
    pub fn create(&self, path: &Path) -> io::Result<()> {
        let file = KeyFile { kind: "x25519".to_string(), seed: Zeroizing::new(URL_SAFE_NO_PAD.encode(*self.seed)) };
        let bytes = Zeroizing::new(serde_json::to_vec(&file).map_err(io::Error::other)?);
        publish_new(path, &bytes, Permissions::Private)
    }

    /// Opens what was sealed to this key with the same `info` and `aad`.
    pub fn open(&self, sealed: &Sealed, info: &[u8], aad: &[u8]) -> Result<Vec<u8>, Unsealable> {
        let encapped = <Kem as hpke::Kem>::EncappedKey::from_bytes(&sealed.enc.0).map_err(|_| Unsealable::Malformed)?;
        hpke::single_shot_open::<ChaCha20Poly1305, HkdfSha256, Kem>(
            &OpModeR::Base,
            &self.secret,
            &encapped,
            info,
            &sealed.ct.0,
            aad,
        )
        .map_err(|_| Unsealable::NotForThisKey)
    }
}

impl fmt::Debug for SealKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SealKey({})", self.public)
    }
}

#[derive(Serialize, Deserialize)]
struct KeyFile {
    kind: String,
    seed: Zeroizing<String>,
}

/// A member's X25519 public key: what others seal to.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SealPublic(pub [u8; 32]);

impl SealPublic {
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Display for SealPublic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&URL_SAFE_NO_PAD.encode(self.0))
    }
}

impl fmt::Debug for SealPublic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SealPublic({self})")
    }
}

impl Serialize for SealPublic {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for SealPublic {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        decode_array::<32>(&text).map(SealPublic).map_err(serde::de::Error::custom)
    }
}

/// Bytes sealed to one member: the encapsulated key and the ciphertext.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sealed {
    pub enc: Bytes,
    pub ct: Bytes,
}

/// The size of an X25519 encapsulated key.
const ENC: usize = 32;

impl Sealed {
    /// The encapsulated key followed by the ciphertext.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(self.enc.0.len() + self.ct.0.len());
        bytes.extend_from_slice(&self.enc.0);
        bytes.extend_from_slice(&self.ct.0);
        bytes
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Unsealable> {
        if bytes.len() < ENC {
            return Err(Unsealable::Malformed);
        }
        let (enc, ct) = bytes.split_at(ENC);
        Ok(Self { enc: Bytes(enc.to_vec()), ct: Bytes(ct.to_vec()) })
    }
}

/// Why sealed bytes could not be opened.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Unsealable {
    #[error("the sealed bytes are malformed")]
    Malformed,
    #[error("it was not sealed to this machine's key, or it was changed on the way")]
    NotForThisKey,
}

/// Seals `plaintext` to `to`. `info` names what it is and between whom;
/// `aad` is what the opener must present unchanged.
pub fn seal(to: &SealPublic, info: &[u8], aad: &[u8], plaintext: &[u8]) -> Result<Sealed, String> {
    let recipient = <Kem as hpke::Kem>::PublicKey::from_bytes(&to.0).map_err(|error| format!("{error:?}"))?;
    let (encapped, ciphertext) =
        hpke::single_shot_seal::<ChaCha20Poly1305, HkdfSha256, Kem>(&OpModeS::Base, &recipient, info, plaintext, aad)
            .map_err(|error| format!("sealing failed: {error:?}"))?;
    Ok(Sealed { enc: Bytes(encapped.to_bytes().to_vec()), ct: Bytes(ciphertext) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sealed_bytes_open_only_with_the_same_key_info_and_aad() {
        let key = SealKey::generate().unwrap();
        let other = SealKey::generate().unwrap();
        let sealed = seal(&key.public(), b"info", b"aad", b"hello").unwrap();
        assert_eq!(key.open(&sealed, b"info", b"aad").unwrap(), b"hello");
        assert_eq!(other.open(&sealed, b"info", b"aad"), Err(Unsealable::NotForThisKey));
        assert_eq!(key.open(&sealed, b"other", b"aad"), Err(Unsealable::NotForThisKey));
        assert_eq!(key.open(&sealed, b"info", b"other"), Err(Unsealable::NotForThisKey));
        let reloaded = SealKey::from_seed(key.seed());
        assert_eq!(reloaded.public(), key.public());
        assert_eq!(reloaded.open(&sealed, b"info", b"aad").unwrap(), b"hello");
    }
}
