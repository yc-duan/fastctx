//! The hub's service key (design §6.1): made with an Ed25519 seed the first
//! time the hub opens, kept in its keyring `keys/` readable by the hub's
//! account alone, beside the keys it retired; a key is retired, never
//! deleted. Everything the hub says of assets is its word under this key,
//! bound to the chain head the witness holds (`fastctx_net::sign::hub_word`).

use super::witness::Witness;
use fastctx_content::v2::HubSigner;
use fastctx_net::sign::{PublicKey, SecretKey, hub_word};
use std::io;
use std::path::Path;
use std::sync::Arc;

/// The key in use.
pub(crate) struct Keys {
    current: SecretKey,
}

/// The file of the key in use within the keyring.
const CURRENT: &str = "current.key";

impl Keys {
    /// The keyring in `dir`, its first key made when there is none.
    pub(crate) fn open(dir: &Path) -> io::Result<Self> {
        fastctx_platform::ensure_private_dir(dir)?;
        let path = dir.join(CURRENT);
        let current = if fastctx_platform::disk::exists(&path)? {
            SecretKey::load(&path)?
        } else {
            let key = SecretKey::generate()?;
            key.create(&path)?;
            key
        };
        Ok(Self { current })
    }

    pub(crate) fn public(&self) -> PublicKey {
        self.current.public()
    }
}

/// Signs as the hub, bound to the chain head the witness holds.
pub(crate) struct Signer {
    pub(crate) keys: Arc<Keys>,
    pub(crate) witness: Arc<Witness>,
}

impl HubSigner for Signer {
    fn sign(&self, statement: &[u8]) -> Vec<u8> {
        let (seq, head) = self.witness.current();
        hub_word(&self.keys.current, seq, &head, statement)
    }
}
