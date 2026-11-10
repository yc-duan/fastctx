//! Replication pipelines independent blocks over reusable data connections bound
//! to one authenticated control incarnation. Byte reservations bound the window.
use super::{Connection, Inner, Link};
use crate::{
    asset_transport::Channel,
    dial,
    sync_transport::Advertisement,
    wire::{Down, Question, Told, Up},
};
use fastctx_sync::{
    admission::{ByteBudget, BytePermit},
    cursor::DurableAck,
    identity::OperationId,
    proof::HubSigningIdentity,
    replicate::{BlockOffer, BlockRequest, Upstream},
    store::{Error, FutureResult},
};
use fastctx_types::sync::{session::*, transfer::Message};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tokio::sync::Mutex;

pub struct AssetUpstream {
    inner: Arc<Inner>,
    identity: HubSigningIdentity,
    budget: ByteBudget,
    channels: Mutex<Vec<Opened>>,
}
struct Opened {
    channel: Channel,
    control: Connection,
    _session: Registered,
}
struct Registered {
    inner: Arc<Inner>,
    control: Connection,
    session: SessionId,
}
impl Drop for Registered {
    fn drop(&mut self) {
        let _ = self.control.up.send(Up::Ask {
            id: self.inner.next_id(),
            question: Question::CloseSyncSession { request: SessionClose { session: self.session.clone() } },
        });
    }
}
impl Link {
    /// The caller supplies the existing content/frame pool. Unsupported peers
    /// are rejected before any new question or asset intent is sent.
    pub async fn asset_upstream(&self, budget: ByteBudget) -> Result<AssetUpstream, Error> {
        let control = self
            .inner
            .connected()
            .await
            .ok_or_else(|| Error::Unavailable("the hub control connection is unavailable".into()))?;
        let remote = control
            .asset_sync
            .as_ref()
            .ok_or_else(|| Error::Unavailable("the hub did not advertise asset sync".into()))?;
        Advertisement::local(None).negotiate(remote).map_err(Error::Unavailable)?;
        let identity = remote
            .identity
            .clone()
            .ok_or_else(|| Error::Unavailable("the hub did not advertise its service identity".into()))?;
        let expected: fastctx_sync::identity::HubId = self.inner.hub.as_str().parse().map_err(Error::Invalid)?;
        if identity.hub != expected {
            return Err(Error::Denied("service signing identity belongs to another hub".into()));
        }
        Ok(AssetUpstream { inner: self.inner.clone(), identity, budget, channels: Mutex::new(Vec::new()) })
    }
}
impl AssetUpstream {
    /// Obtained from the certificate-pinned authenticated control connection.
    /// The consumer pins it before using its proofs as a directory authority.
    pub fn identity(&self) -> &HubSigningIdentity {
        &self.identity
    }
    async fn open(&self, prepared_frames: Option<BytePermit>) -> Result<Opened, Error> {
        let control = self
            .inner
            .connected()
            .await
            .ok_or_else(|| Error::Unavailable("the hub control connection is unavailable".into()))?;
        let remote = control
            .asset_sync
            .as_ref()
            .ok_or_else(|| Error::Unavailable("the hub did not advertise asset sync".into()))?;
        if remote.identity.as_ref() != Some(&self.identity) {
            return Err(Error::Unavailable("the hub signing identity changed; rebind the directory consumer".into()));
        }
        let selected = Advertisement::local(None).negotiate(remote).map_err(Error::Unavailable)?;
        let session = SessionId::from_bytes(crate::sign::random::<16>().map_err(crate::asset_transport::io_error)?);
        let registered = Registered { inner: self.inner.clone(), control: control.clone(), session: session.clone() };
        let request = SessionOpen {
            session: session.clone(),
            binding: SessionBindingRequest::Hub { hub: self.identity.hub.clone() },
            limits: selected.limits().clone(),
        };
        let answer = self
            .inner
            .ask_on(control.clone(), Question::OpenSyncSession { request })
            .await
            .map_err(Error::Unavailable)?;
        let Some(Told::SyncSessionReady { ready }) = answer else {
            return Err(Error::Unavailable("the hub did not open an asset session".into()));
        };
        if ready.session != session || ready.holding.is_some() || &ready.limits != selected.limits() {
            return Err(Error::Invalid("the asset ready reply differs from the hub-bound request".into()));
        }
        let frames = match prepared_frames {
            Some(frames) => frames,
            None => {
                self.budget
                    .acquire_until(
                        crate::asset_transport::frame_bytes(&selected),
                        tokio::time::Instant::now() + crate::wire::SILENCE,
                    )
                    .await?
            }
        };
        let path = format!("{}/{}", crate::wire::ASSET_SYNC_PATH, session.as_str());
        let mut data = dial::dial_upstream_configured(
            &control.upstream,
            &self.inner.hub,
            self.inner.cert,
            &path,
            selected.websocket_config(),
        )
        .await
        .map_err(Error::Unavailable)?;
        let hello = Up::Hello {
            protocol: crate::wire::GREETING_PROTOCOL,
            version: env!("CARGO_PKG_VERSION").into(),
            intent: selected.hub_intent(
                session.clone(),
                self.inner.key.public(),
                self.inner.key.sign(crate::sign::Domain::Session, &data.binding),
            ),
        };
        dial::send(&mut data.socket, &hello).await.map_err(Error::Unavailable)?;
        match dial::recv(&mut data.socket).await.map_err(Error::Unavailable)? {
            Some(Down::Paired) => {}
            _ => return Err(Error::Unavailable("the hub refused the bound asset data connection".into())),
        }
        let channel = Channel::new(data.socket, selected, session, self.budget.clone(), frames).await?;
        Ok(Opened { channel, control, _session: registered })
    }
    async fn block_window(&self) -> Result<(BytePermit, BytePermit), Error> {
        // Admit a complete window once. Joining the FIFO again while holding its
        // work bytes could deadlock behind another complete window.
        let frame_bytes = 4 * fastctx_types::sync::session::MAX_FRAME_BYTES as usize;
        let mut memory = self
            .budget
            .acquire_until(
                crate::asset_transport::parts::WORKING_BYTES + frame_bytes,
                tokio::time::Instant::now() + crate::wire::SILENCE,
            )
            .await?;
        let frames = memory.split(frame_bytes)?;
        Ok((memory, frames))
    }
    async fn request<T>(
        &self,
        operation: Option<OperationId>,
        prepared_frames: Option<BytePermit>,
        work: impl for<'a> FnOnce(&'a mut Channel) -> FutureResult<'a, T>,
    ) -> Result<T, Error> {
        let attempted = Arc::new(AtomicBool::new(false));
        let refused = Arc::new(AtomicBool::new(false));
        let result = async {
            let reusable = {
                let mut stored = self.channels.lock().await;
                let generation = self.inner.connection().map(|now| now.generation);
                stored
                    .retain(|old| generation == Some(old.control.generation) && old.control.closed.borrow().is_none());
                stored.pop()
            };
            let mut opened = match reusable {
                Some(old) => {
                    drop(prepared_frames);
                    old
                }
                None => tokio::time::timeout(crate::wire::SILENCE, self.open(prepared_frames))
                    .await
                    .map_err(|_| Error::Unavailable("asset connection opening deadline elapsed".into()))??,
            };
            opened.channel.attempted = attempted.clone();
            opened.channel.refused = refused.clone();
            let mut closed = opened.control.closed.clone();
            let result = tokio::select! {
                _ = closed.changed() => Err(Error::Unavailable("the authenticated control connection ended".into())),
                result = work(&mut opened.channel) => result,
            };
            if result.is_ok() {
                // Idle frame buffers must leave room for complete work windows.
                let retained = (self.budget.capacity()
                    / (crate::asset_transport::parts::WORKING_BYTES
                        + 4 * fastctx_types::sync::session::MAX_FRAME_BYTES as usize))
                    .max(1);
                let mut stored = self.channels.lock().await;
                if stored.len() < retained {
                    stored.push(opened);
                }
            }
            result
        }
        .await;
        if let (Err(error), Some(operation)) = (&result, operation)
            && attempted.load(Ordering::Acquire)
            && !refused.load(Ordering::Acquire)
        {
            self.inner.node.log(format!("asset operation {} has an unconfirmed outcome: {error}", operation.as_str()));
            return Err(Error::OutcomeUnknown { operation });
        }
        result
    }
}
impl Upstream for AssetUpstream {
    fn exchange(&self, request: Message) -> FutureResult<'_, fastctx_sync::replicate::Received<Message>> {
        Box::pin(async move {
            let operation = match &request {
                Message::Source { batch, .. } => {
                    serde_json::from_slice::<SourceOperation>(&batch.proof.data.0).ok().map(|batch| batch.operation)
                }
                Message::Submit { request } => Some(request.operation.clone()),
                _ => None,
            };
            self.request(operation, None, move |channel| {
                Box::pin(crate::asset_transport::client::exchange(channel, request))
            })
            .await
        })
    }
    fn fetch(&self, request: BlockRequest) -> FutureResult<'_, fastctx_sync::replicate::Received<Vec<u8>>> {
        Box::pin(async move {
            let (memory, frames) = self.block_window().await?;
            self.request(None, Some(frames), move |channel| {
                Box::pin(crate::asset_transport::client::fetch_prepared(channel, request, memory))
            })
            .await
        })
    }
    fn offer(&self, offer: BlockOffer) -> FutureResult<'_, DurableAck> {
        Box::pin(async move {
            let (memory, frames) = self.block_window().await?;
            self.request(Some(offer.operation.clone()), Some(frames), move |channel| {
                Box::pin(crate::asset_transport::client::offer_prepared(channel, offer, memory))
            })
            .await
        })
    }
}
#[derive(serde::Deserialize)]
struct SourceOperation {
    operation: OperationId,
}
