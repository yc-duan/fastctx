//! A device learns the scopes its hub describes for it: it pages through
//! discovery, verifies every description against the pinned hub key and
//! installs it through the one scope entry.
use super::Upstream;
use crate::{
    cursor::ScopeCursor,
    proof::{HubSigningIdentity, ProofVerifier},
    scope::{self, ScopeDiscoveryRead},
    store::{
        Error,
        scopes::{Installed, SqliteScopes},
    },
    transfer::Message,
};
use std::sync::Arc;

/// What one complete discovery installed, and the directory feeds the device
/// follows from here on.
#[derive(Clone, Debug)]
pub struct Discovered {
    pub installed: Vec<Installed>,
    pub directories: Vec<ScopeCursor>,
}

pub struct DirectoryFollower {
    scopes: SqliteScopes,
    identity: HubSigningIdentity,
    verifier: Arc<dyn ProofVerifier>,
}

impl DirectoryFollower {
    /// `identity` is the hub key this device pinned over its authenticated
    /// connection; `verifier` checks signatures against it.
    pub fn new(scopes: SqliteScopes, identity: HubSigningIdentity, verifier: Arc<dyn ProofVerifier>) -> Self {
        Self { scopes, identity, verifier }
    }

    /// Pages through the hub's discovery once and installs every description.
    /// A description that fails verification stops the walk; those installed
    /// before it stay, each having been verified on its own.
    pub async fn discover(&self, upstream: &dyn Upstream) -> Result<Discovered, Error> {
        let mut installed = Vec::new();
        let mut cursor = None;
        loop {
            let request = ScopeDiscoveryRead { cursor, directories: Vec::new(), max_items: 64, max_bytes: 256 * 1024 };
            let Message::ScopesPage { page } = upstream.exchange(Message::GetScopes { request }).await? else {
                return Err(Error::Invalid("the hub answered discovery with another message".into()));
            };
            for offer in page.entries {
                let verified =
                    scope::verify(&offer.cursor.scope, offer.binding, &self.identity, self.verifier.as_ref())?;
                installed.push(self.scopes.install(&verified).await?);
            }
            match page.next {
                Some(next) => cursor = Some(next),
                None => return Ok(Discovered { installed, directories: page.directories }),
            }
        }
    }
}
