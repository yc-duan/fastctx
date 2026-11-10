//! A hub's side of asset sync: one service answers each authenticated device's
//! requests from the hub's engines. The transport builds the [`Peer`] from the
//! authenticated connection; no field in a request grants a permission.
mod changes;
mod epoch;
mod read;
mod source_transfer;
mod sources;
mod stream;
mod supply;
pub use sources::PeerUpload;

use crate::{
    identity::{AccountId, DeviceId},
    scope::ScopeDescriptor,
    store::{
        Error, FutureResult,
        authority::SqliteAuthority,
        directory::{Recipient, SqliteDirectory},
    },
    transfer::Message,
};
use std::sync::Arc;

/// Authenticated device and accounts, supplied by the transport.
#[derive(Clone, Debug)]
pub struct Peer {
    pub device: DeviceId,
    pub accounts: Vec<AccountId>,
}

/// Additional visibility restrictions over a current, trusted description.
/// The service always requires the owner account in `Peer::accounts` first.
pub trait ReadPolicy: Send + Sync {
    fn can_read(&self, peer: &Peer, descriptor: &ScopeDescriptor) -> bool;
}
struct OwnerAccounts;
impl ReadPolicy for OwnerAccounts {
    fn can_read(&self, _: &Peer, _: &ScopeDescriptor) -> bool {
        true
    }
}

pub struct Service {
    directory: Arc<SqliteDirectory>,
    sources: Option<Arc<SqliteAuthority>>,
    policy: Arc<dyn ReadPolicy>,
}
impl Service {
    pub fn new(directory: Arc<SqliteDirectory>) -> Self {
        Self { directory, sources: None, policy: Arc::new(OwnerAccounts) }
    }

    /// Adds the installed source receiver; absent kinds still remain unavailable.
    pub fn with_sources(mut self, sources: Arc<SqliteAuthority>) -> Self {
        self.sources = Some(sources);
        self
    }
    pub fn with_read_policy(mut self, policy: Arc<dyn ReadPolicy>) -> Self {
        self.policy = policy;
        self
    }
    /// Answers one request from `peer`. Read responses are bounded including
    /// their complete JSON envelope.
    pub fn answer<'a>(&'a self, peer: &'a Peer, request: Message) -> FutureResult<'a, Message> {
        Box::pin(async move {
            match request {
                Message::GetScopes { request } => {
                    let recipient = Recipient { device: peer.device.clone(), accounts: peer.accounts.clone() };
                    Ok(Message::ScopesPage { page: self.directory.discover(recipient, request).await? })
                }
                Message::ProbeSource { upload, content } => self.probe_source(peer, upload, content).await,
                Message::PublishSourceManifest { upload, page } => {
                    self.publish_source_manifest(peer, upload, page).await
                }
                Message::Source { holding, batch } => {
                    Ok(Message::SourceResult { result: self.accept_source(peer, holding, batch).await? })
                }
                Message::Changes { holding, after, view, max_bytes } => {
                    let max = read::budget(max_bytes)?;
                    self.read(peer.clone(), after.scope.clone(), Some(holding.clone()), move |db, descriptor| {
                        changes::page(db, descriptor, holding, after, view, max)
                    })
                    .await
                }
                Message::StreamRange { holding, request } => {
                    let max = read::budget(request.max_bytes)?;
                    if request.max_records == 0 || request.max_records > 256 {
                        return Err(Error::Invalid("a stream page holds 1 to 256 records".into()));
                    }
                    self.read(peer.clone(), request.stream.scope.clone(), Some(holding.clone()), move |db, _| {
                        stream::page(db, holding, request, max)
                    })
                    .await
                }
                Message::GetManifest { holding, root } => self.manifest(peer, holding, root).await,
                Message::GetDescriptor { holding, blob } => self.descriptor(peer, holding, blob).await,
                Message::WantBlocks { holding, manifest, missing } => {
                    self.wanted(peer, holding, manifest, missing).await
                }
                Message::GetEpoch { scope, lookup } => {
                    self.read(peer.clone(), scope.clone(), None, move |db, _| {
                        let descriptor = epoch::get(db, &scope, &lookup)?;
                        let answer = Message::Epoch { descriptor };
                        if read::size(&answer)? > fastctx_types::sync::PAGE_BYTES as usize {
                            return Err(read::unavailable("the epoch proof exceeds the metadata page budget"));
                        }
                        Ok(answer)
                    })
                    .await
                }
                _ => Err(Error::Unavailable("this hub does not serve that request yet".into())),
            }
        })
    }
}
