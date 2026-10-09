//! What tells hubs apart and how a machine knows it reached the right one.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::sign::{Digest, sha256};

pub use fastctx_types::access::HubId;

/// A new hub's id, at random.
pub fn new_hub_id() -> std::io::Result<HubId> {
    Ok(HubId::from_bytes(crate::sign::random::<8>()?))
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
