//! What tells hubs apart and how a machine knows it reached the right one.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::sign::{Digest, sha256};

/// Which hub: chosen when the hub is made, and named in everything said
/// through it, so nothing meant for one hub is taken by another.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct WorldId(String);

impl WorldId {
    pub fn generate() -> std::io::Result<Self> {
        Ok(Self::from_bytes(crate::sign::random::<8>()?))
    }

    pub fn from_bytes(bytes: [u8; 8]) -> Self {
        Self(format!("w-{}", bytes.iter().map(|byte| format!("{byte:02x}")).collect::<String>()))
    }

    /// The eight bytes the id spells.
    pub fn bytes(&self) -> [u8; 8] {
        let mut bytes = [0_u8; 8];
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&self.0[2 + index * 2..4 + index * 2], 16).expect("checked when made");
        }
        bytes
    }
}

impl TryFrom<String> for WorldId {
    type Error = String;

    fn try_from(text: String) -> Result<Self, String> {
        let ok = text.len() == 18
            && text.starts_with("w-")
            && text[2..].bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        if ok { Ok(Self(text)) } else { Err(format!("{text:?} is not a hub id")) }
    }
}

impl From<WorldId> for String {
    fn from(id: WorldId) -> String {
        id.0
    }
}

impl fmt::Display for WorldId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for WorldId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.0, f)
    }
}

/// The hub's certificate, by the SHA-256 of its DER bytes. Machines reach
/// the hub by its IP address and accept exactly this certificate once they
/// learned it: from an enrollment token, or from a login that proved the
/// hub knows the account's password file (`crate::login`).
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CertPin(pub Digest);

impl CertPin {
    pub fn of(certificate_der: &[u8]) -> Self {
        Self(Digest(sha256(certificate_der)))
    }
}

impl fmt::Debug for CertPin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CertPin({})", self.0)
    }
}

impl fmt::Display for CertPin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}
