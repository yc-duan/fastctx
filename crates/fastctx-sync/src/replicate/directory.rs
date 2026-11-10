//! A device learns the scopes its hub describes for it: it pages through
//! discovery, verifies every description against the pinned hub key and
//! installs it through the one scope entry.
use super::Upstream;
use crate::{
    cursor::ScopeCursor,
    proof::{HubSigningIdentity, ProofVerifier},
    scope::{self, DiscoveryCursor, ScopeDiscoveryRead, ScopeOffer},
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

/// One verified and installed discovery page, retained for bounded enrollment by
/// the replica driver. The offers' cursors describe the hub, not local progress.
pub struct DiscoveredPage {
    pub installed: Vec<(ScopeOffer, Installed)>,
    pub directories: Vec<ScopeCursor>,
    pub next: Option<DiscoveryCursor>,
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

    pub async fn page(
        &self,
        upstream: &dyn Upstream,
        cursor: Option<DiscoveryCursor>,
    ) -> Result<DiscoveredPage, Error> {
        let request = ScopeDiscoveryRead {
            cursor: cursor.clone(),
            directories: Vec::new(),
            max_items: 64,
            max_bytes: 256 * 1024,
        };
        let answer = upstream.exchange(Message::GetScopes { request }).await?;
        if serde_json::to_vec(&answer).map_err(|e| Error::Invalid(e.to_string()))?.len()
            > fastctx_types::sync::PAGE_BYTES as usize
        {
            return Err(Error::Invalid("discovery exceeded its byte budget".into()));
        }
        let Message::ScopesPage { page } = answer else {
            return Err(Error::Invalid("the hub answered discovery with another message".into()));
        };
        if page.entries.len() > 64
            || &page.fence.recipient != self.scopes.device()
            || cursor.as_ref().is_some_and(|previous| previous.fence != page.fence)
            || page.entries.windows(2).any(|pair| pair[0].cursor.scope >= pair[1].cursor.scope)
            || page.entries.iter().any(|offer| {
                offer.cursor.scope.hub != self.identity.hub
                    || cursor.as_ref().is_some_and(|previous| offer.cursor.scope <= previous.after)
            })
            || page.directories.iter().any(|cursor| cursor.scope.hub != self.identity.hub)
        {
            return Err(Error::Invalid("discovery differs from its recipient, fence or ordered range".into()));
        }
        if let Some(next) = &page.next
            && (next.fence != page.fence
                || page.entries.last().map(|offer| &offer.cursor.scope) != Some(&next.after)
                || !page.directories.is_empty())
        {
            return Err(Error::Invalid("discovery continuation made no ordered progress".into()));
        }
        let mut installed = Vec::new();
        for offer in page.entries {
            let verified =
                scope::verify(&offer.cursor.scope, offer.binding.clone(), &self.identity, self.verifier.as_ref())?;
            if offer.cursor.acl_revision != verified.descriptor().acl_revision {
                return Err(Error::Invalid("discovery cursor names another scope ACL".into()));
            }
            installed.push((offer, self.scopes.install(&verified).await?));
        }
        Ok(DiscoveredPage { installed, directories: page.directories, next: page.next })
    }

    /// Pages through the hub's discovery once and installs every description.
    /// A description that fails verification stops the walk; those installed
    /// before it stay, each having been verified on its own.
    pub async fn discover(&self, upstream: &dyn Upstream) -> Result<Discovered, Error> {
        let mut installed = Vec::new();
        let mut cursor = None;
        loop {
            let page = self.page(upstream, cursor).await?;
            installed.extend(page.installed.into_iter().map(|(_, state)| state));
            match page.next {
                Some(next) => cursor = Some(next),
                None => return Ok(Discovered { installed, directories: page.directories }),
            }
        }
    }
}
