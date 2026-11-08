//! Stable identifiers; display names and runtime addresses are not object keys.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

pub use crate::access::HubId;

macro_rules! identifier {
    ($name:ident, $prefix:literal, $description:literal) => {
        identifier!($name, $prefix, $description, 32);
    };
    ($name:ident, $prefix:literal, $description:literal, $bytes:literal) => {
        #[doc = $description]
        #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);

        impl $name {
            pub fn from_bytes(bytes: [u8; $bytes]) -> Self {
                Self(format!("{}{}", $prefix, hex(&bytes)))
            }
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
        impl TryFrom<String> for $name {
            type Error = String;
            fn try_from(text: String) -> Result<Self, Self::Error> {
                let Some(digest) = text.strip_prefix($prefix) else {
                    return Err(concat!("invalid ", stringify!($name)).into());
                };
                if digest.len() != 2 * $bytes
                    || !digest.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                {
                    return Err(concat!("invalid ", stringify!($name)).into());
                }
                Ok(Self(text))
            }
        }
        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.0
            }
        }
    };
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

identifier!(AccountId, "a-", "An account incarnation, independent of its display name.");
identifier!(DeviceId, "d-", "The bytes of an existing device public key, not a machine name.");
identifier!(ScopeId, "s-", "A stable ownership policy within one hub.");
identifier!(ObjectId, "o-", "An object registered once by its authorized creator.");
identifier!(StreamId, "r-", "A single sequenced source within a scope.");
identifier!(ProducerEpoch, "p-", "A durable producer incarnation, changed on transfer or state loss.");
identifier!(FeedEpoch, "f-", "A hub catalog incarnation, changed when its sequence space changes.");
identifier!(OperationId, "m-", "An idempotency key reused for every retry of one operation.");
identifier!(SourceId, "i-", "A registered ingestion source, unrelated to its current filesystem path.");
identifier!(Digest, "", "A lowercase SHA-256 digest.");
identifier!(
    AuthorityEpoch,
    "e-",
    "A random 128-bit incarnation persisted before an authority accepts writes after startup.",
    16
);
identifier!(ViewId, "v-", "The negotiated kind/schema set and ACL projection of a scope.");

impl AuthorityEpoch {
    pub fn bytes(&self) -> [u8; 16] {
        let text = self.0.strip_prefix("e-").expect("validated authority epoch");
        std::array::from_fn(|index| {
            u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).expect("validated authority epoch")
        })
    }
}

impl AccountId {
    /// Maps legacy membership once. The mapping must be saved before publication;
    /// later renames retain it. Account recreation has a different opening time.
    pub fn from_legacy(hub: &HubId, name: &crate::AccountName, opened: Timestamp) -> Self {
        let mut hash = Sha256::new();
        hash.update(b"fastctx.account-incarnation.v1\0");
        hash.update(hub.bytes());
        let name = name.to_string();
        hash.update((name.len() as u64).to_be_bytes());
        hash.update(name.as_bytes());
        hash.update(opened.as_nanosecond().to_be_bytes());
        Self::from_bytes(hash.finalize().into())
    }
}

impl Digest {
    pub fn bytes(&self) -> [u8; 32] {
        std::array::from_fn(|index| {
            u8::from_str_radix(&self.0[index * 2..index * 2 + 2], 16).expect("validated digest")
        })
    }
    pub fn of(bytes: &[u8]) -> Self {
        Self::from_bytes(Sha256::digest(bytes).into())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ScopeKey {
    pub hub: HubId,
    pub scope: ScopeId,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ObjectKey {
    pub scope: ScopeKey,
    pub object: ObjectId,
}
