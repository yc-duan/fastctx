//! Asset sessions are opened on the authenticated control connection; a data
//! socket must prove the same device over its own certificate-pinned TLS stream.
use crate::{Hub, Socket};
use fastctx_net::{
    asset_transport::{
        server::{self, Endpoint},
        sessions::BoundSession,
    },
    sign::{PublicKey, Signature},
    wire::{self, Down, Told},
};
use fastctx_sync::{
    authority::{Peer, PeerUpload},
    blocks::codec::EncodedBlock,
    identity::{AccountId, DeviceId},
    manifest::{PublishedBlock, Representation},
    store::{Error, FutureResult},
};
use fastctx_types::{MachineName, sync::session::*};
use std::sync::Arc;

pub(crate) struct Opening {
    pub config: tokio_tungstenite::tungstenite::protocol::WebSocketConfig,
    pub frames: fastctx_sync::admission::BytePermit,
}
pub(crate) fn configuration(hub: &Hub, path: &str) -> Result<Option<Opening>, Error> {
    let Some(id) = path.strip_prefix(&format!("{}/", wire::ASSET_SYNC_PATH)) else { return Ok(None) };
    let Ok(id) = SessionId::try_from(id.to_owned()) else { return Ok(None) };
    let Some(foundation) = hub.foundation.as_ref() else { return Ok(None) };
    let Some(bound) = foundation.asset_sessions().get(&id) else { return Ok(None) };
    // Every attempted socket needs its own reservation, even when several peers
    // use the same path and have not yet supplied the TLS-bound device proof.
    let frames =
        foundation.asset_sessions().budget().try_acquire(fastctx_net::asset_transport::frame_bytes(&bound.selected))?;
    Ok(Some(Opening { config: bound.selected.websocket_config(), frames }))
}
pub(crate) fn open(
    hub: &Arc<Hub>,
    member: &MachineName,
    key: PublicKey,
    generation: u64,
    request: SessionOpen,
) -> Result<Told, String> {
    let foundation = hub.foundation.as_ref().ok_or("the data foundation is unavailable")?;
    let remote = hub
        .sessions()
        .get(member)
        .filter(|s| s.key == key && s.generation == generation)
        .and_then(|s| s.asset_sync.clone())
        .ok_or("asset sync was not advertised on this control connection")?;
    let local = foundation.advertisement().ok_or("the data foundation has not begun")?;
    let mut selected = local.negotiate(&remote)?;
    selected.restrict(&request.limits)?;
    let identity = local.identity.as_ref().ok_or("the hub has no service signing identity")?;
    match &request.binding {
        SessionBindingRequest::Hub { hub } if hub == &identity.hub => {}
        SessionBindingRequest::Hub { .. } => return Err("asset session belongs to another hub".into()),
        _ => return Err("scope-bound session readiness is not installed".into()),
    }
    let ready = SessionReady { session: request.session, holding: None, limits: selected.limits().clone() };
    foundation
        .asset_sessions()
        .open(member.clone(), key, generation, request.binding, ready.clone(), selected)
        .map_err(|e| e.to_string())?;
    Ok(Told::SyncSessionReady { ready })
}
pub(crate) async fn connected(
    hub: Arc<Hub>,
    mut socket: Socket,
    path: String,
    exporter: [u8; 32],
    session: SessionId,
    identity: (PublicKey, Signature),
    frames: fastctx_sync::admission::BytePermit,
) {
    let (key, proof) = identity;
    let result = async {
        if path != format!("{}/{}", wire::ASSET_SYNC_PATH, session.as_str()) {
            return Err(Error::Denied("asset path and session differ".into()));
        }
        let member = hub
            .member(&key, &proof, &exporter)
            .map_err(|_| Error::Denied("asset TLS device proof was refused".into()))?;
        let foundation =
            hub.foundation.as_ref().ok_or_else(|| Error::Unavailable("the data foundation is unavailable".into()))?;
        let attachment = foundation.asset_sessions().attach(&session, key)?;
        if attachment.0.member != member {
            return Err(Error::Denied("asset member changed".into()));
        }
        let endpoint =
            Service { hub: hub.clone(), bound: attachment.0.clone(), peer_cache: std::sync::Mutex::new(None) };
        endpoint.current()?;
        crate::send(&mut socket, &Down::Paired).await.map_err(Error::Unavailable)?;
        server::run(socket, attachment, &endpoint, foundation.asset_sessions().budget().clone(), frames).await
    }
    .await;
    if let Err(error) = result {
        hub.log.line(format_args!("asset data connection: {error}"));
    }
}
struct Service {
    hub: Arc<Hub>,
    bound: Arc<BoundSession>,
    peer_cache: std::sync::Mutex<Option<(u64, Peer, fastctx_sync::admission::BytePermit)>>,
}
impl Service {
    fn peer(&self) -> Result<Peer, Error> {
        self.current()?;
        let store = self.hub.store();
        let mut cache = self.peer_cache.lock().expect("never poisoned");
        if let Some((sequence, peer, _permit)) = &*cache
            && *sequence == store.sequence
        {
            return Ok(peer.clone());
        }
        *cache = None;
        let directory = &store.book.directory;
        let hub = store.hub.id.clone();
        let authorized = |account: &&Arc<fastctx_net::directory::Account>| {
            store
                .indexed()
                .authorize(&account.name, fastctx_net::authorization::Action::ActFor(&self.bound.member))
                .is_ok()
        };
        let count = directory.accounts.iter().filter(authorized).count();
        // Account IDs have 66 ASCII bytes. Cover vector growth, string capacity,
        // the retained peer, and snapshot clones during one sequential request.
        let bytes = count
            .checked_mul((132 + 2 * std::mem::size_of::<AccountId>()) * 8)
            .and_then(|n| n.checked_add(2048))
            .ok_or(Error::Busy)?;
        let permit = self
            .hub
            .foundation
            .as_ref()
            .ok_or_else(|| Error::Unavailable("the data foundation is unavailable".into()))?
            .asset_sessions()
            .budget()
            .try_acquire(bytes)?;
        let accounts = directory
            .accounts
            .iter()
            .filter(authorized)
            .map(|account| AccountId::from_legacy(&hub, &account.name, account.opened))
            .collect();
        let peer = Peer { device: DeviceId::from_bytes(*self.bound.key.as_bytes()), accounts };
        *cache = Some((store.sequence, peer.clone(), permit));
        Ok(peer)
    }
    fn download(request: &TransferRequest) -> Result<fastctx_sync::replicate::BlockRequest, Error> {
        let TransferTarget::Single { content } = &request.target else {
            return Err(Error::Unavailable("manifest range transfer is not installed".into()));
        };
        Ok(fastctx_sync::replicate::BlockRequest {
            holding: request.holding.clone(),
            object: request.object.clone(),
            kind: request.kind.clone(),
            root: request.root.as_ref().clone(),
            block: content.clone(),
        })
    }
    fn upload(request: &TransferRequest) -> Result<PeerUpload, Error> {
        let Some(UploadCause::Source { epoch }) = &request.upload_cause else {
            return Err(Error::Unavailable("mutation staging is not installed".into()));
        };
        let kind = fastctx_sync::registry::REGISTRY.asset(&request.kind.kind).map_err(Error::Invalid)?;
        if kind.registration().schema != request.kind.schema {
            return Err(Error::Invalid("source upload schema differs from the registry".into()));
        }
        Ok(PeerUpload {
            holding: request.holding.clone(),
            kind: request.kind.kind.clone(),
            epoch: epoch.clone(),
            operation: request.operation.clone(),
        })
    }
}
impl Endpoint for Service {
    fn current(&self) -> Result<(), Error> {
        if *self.bound.stopped.borrow() || self.bound.stopped.has_changed().is_err() || self.hub.moving.frozen() {
            return Err(Error::Unavailable("asset control session ended".into()));
        }
        if self
            .hub
            .sessions()
            .get(&self.bound.member)
            .is_none_or(|s| s.generation != self.bound.generation || s.key != self.bound.key)
        {
            return Err(Error::Denied("asset control incarnation was replaced".into()));
        }
        let store = self.hub.store();
        if store.hub.dissolved.is_some() || store.indexed().by_key(&self.bound.key).is_none() {
            return Err(Error::Denied("asset device is no longer a member".into()));
        }
        Ok(())
    }
    fn answer(
        &self,
        request: fastctx_types::sync::transfer::Message,
    ) -> FutureResult<'_, fastctx_types::sync::transfer::Message> {
        Box::pin(async move {
            // Source commits join the handover fence; a frozen check before an
            // awaited transaction alone would let acknowledged writes miss it.
            let _active = if matches!(
                &request,
                fastctx_sync::transfer::Message::Source { .. } | fastctx_sync::transfer::Message::Submit { .. }
            ) {
                Some(
                    crate::moving::Moving::enter(&self.hub)
                        .ok_or_else(|| Error::Unavailable("the hub is handing over".into()))?,
                )
            } else {
                None
            };
            let peer = self.peer()?;
            self.hub
                .foundation
                .as_ref()
                .ok_or_else(|| Error::Unavailable("the data foundation is unavailable".into()))?
                .answer(&peer, request)
                .await
        })
    }
    fn recheck_message<'a>(&'a self, request: &'a fastctx_sync::transfer::Message) -> FutureResult<'a, ()> {
        Box::pin(async move {
            let peer = self.peer()?;
            self.hub
                .foundation
                .as_ref()
                .ok_or_else(|| Error::Unavailable("the data foundation is unavailable".into()))?
                .recheck_message(&peer, request)
                .await
        })
    }
    fn authorize_transfer<'a>(&'a self, request: &'a TransferRequest) -> FutureResult<'a, ()> {
        Box::pin(async move {
            let peer = self.peer()?;
            if request.direction == TransferDirection::Download {
                let foundation = self
                    .hub
                    .foundation
                    .as_ref()
                    .ok_or_else(|| Error::Unavailable("the data foundation is unavailable".into()))?;
                foundation.recheck(&peer, &request.holding).await?;
                return foundation.check_supply(&peer, &Self::download(request)?).await;
            }
            let upload = Self::upload(request)?;
            self.hub
                .foundation
                .as_ref()
                .ok_or_else(|| Error::Unavailable("the data foundation is unavailable".into()))?
                .check_source_block(&peer, &request.object, &upload)
                .await
        })
    }
    fn get<'a>(&'a self, request: &'a TransferRequest) -> FutureResult<'a, EncodedBlock> {
        Box::pin(async move {
            let peer = self.peer()?;
            let foundation = self
                .hub
                .foundation
                .as_ref()
                .ok_or_else(|| Error::Unavailable("the data foundation is unavailable".into()))?;
            let body = foundation.supply(&peer, Self::download(request)?).await?;
            let encoding = match &request.representation {
                RepresentationChoice::Encodings { preferred } => {
                    preferred.iter().copied().find(|encoding| self.bound.selected.supports_encoding(*encoding))
                }
                RepresentationChoice::Exact { .. } => None,
            }
            .ok_or_else(|| Error::Unavailable("no supplied representation was negotiated".into()))?;
            let block =
                fastctx_sync::blocks::codec::encode(request.object.scope.clone(), body.value().clone(), encoding)?;
            foundation.recheck(&peer, &request.holding).await?;
            Ok(block)
        })
    }
    fn put<'a>(
        &'a self,
        request: &'a TransferRequest,
        representation: Representation,
        encoded: Vec<u8>,
    ) -> FutureResult<'a, PublishedBlock> {
        Box::pin(async move {
            let _active = crate::moving::Moving::enter(&self.hub)
                .ok_or_else(|| Error::Unavailable("the hub is handing over".into()))?;
            let peer = self.peer()?;
            let upload = Self::upload(request)?;
            let foundation = self
                .hub
                .foundation
                .as_ref()
                .ok_or_else(|| Error::Unavailable("the data foundation is unavailable".into()))?;
            foundation.check_source_block(&peer, &request.object, &upload).await?;
            foundation.put_source_block(&peer, upload, representation, encoded).await
        })
    }
}
