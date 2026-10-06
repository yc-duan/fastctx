//! The World's naming key, and what it names.
//!
//! The wire never carries a project's name: a project is known there by an
//! id worked out from its name with a key every member holds and the hub
//! does not. The same name gives the same id on every machine, so two
//! machines opening a project of the same name open the same one, and the
//! hub, which keeps the first entry of each, lets only one of them do it.
//! The paths an entry writes reach the hub the same way, as tokens, so the
//! hub can refuse a path written twice without learning what it is.
//!
//! The key names things and seals nothing: it never changes, and a member
//! that loses its scope keeps what little it tells, which is whether two
//! names or two paths are the same. It is made once, when the World is
//! founded or first moved to this version, and handed to every member in
//! the World's own ledger, sealed to each.

use std::fmt;

use fastctx_types::TownName;
use hmac::{Hmac, Mac as _};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::Sha256;
use zeroize::Zeroizing;

use super::TownId;
use crate::sign::Bytes;

#[derive(Clone)]
pub struct NamingKey(Zeroizing<[u8; 32]>);

impl NamingKey {
    pub fn generate() -> std::io::Result<Self> {
        Ok(Self(Zeroizing::new(crate::sign::random::<32>()?)))
    }

    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(Zeroizing::new(bytes))
    }

    fn mac(&self, parts: &[&[u8]]) -> [u8; 32] {
        let mut mac = <Hmac<Sha256> as hmac::Mac>::new_from_slice(&*self.0).expect("HMAC takes any key");
        for part in parts {
            mac.update(part);
            mac.update(&[0]);
        }
        mac.finalize().into_bytes().into()
    }
}

impl fmt::Debug for NamingKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("NamingKey(..)")
    }
}

impl PartialEq for NamingKey {
    fn eq(&self, other: &Self) -> bool {
        *self.0 == *other.0
    }
}

impl Eq for NamingKey {}

impl Serialize for NamingKey {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        crate::sign::Secret(self.0.clone()).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for NamingKey {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        crate::sign::Secret::deserialize(deserializer).map(|secret| Self(secret.0))
    }
}

/// The id of the project named `town`.
pub fn town_id(key: &NamingKey, town: &TownName) -> TownId {
    let mac = key.mac(&[b"town", town.as_bytes()]);
    TownId::from_bytes(mac[..8].try_into().expect("eight bytes"))
}

/// What the hub knows of a path in a ledger: equal for paths that are the
/// same on a system that ignores case, and nothing else.
pub type PathToken = Bytes;

/// The token of `path` in the ledger of `town`.
pub fn path_token(key: &NamingKey, town: &TownId, path: &str) -> PathToken {
    let mac = key.mac(&[b"path", town.as_str().as_bytes(), path.to_lowercase().as_bytes()]);
    Bytes(mac[..16].to_vec())
}
