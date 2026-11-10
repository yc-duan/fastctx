//! Server-side framing delegates every permission and durable action to its endpoint.
use super::{
    channel::{Channel, Event},
    parts,
    sessions::Attachment,
};
use fastctx_sync::{
    blocks::codec::EncodedBlock,
    manifest::PublishedBlock,
    store::{Error, FutureResult},
};
use fastctx_types::sync::{cursor::DurableAck, manifest::Representation, session::*, transfer::Message};

/// Implemented by service composition over the current authenticated directory.
/// A binding, transfer ID or offered byte never grants an object permission.
pub trait Endpoint: Send + Sync {
    fn current(&self) -> Result<(), Error>;
    fn answer(&self, request: Message) -> FutureResult<'_, Message>;
    fn recheck_message<'a>(&'a self, request: &'a Message) -> FutureResult<'a, ()>;
    fn authorize_transfer<'a>(&'a self, request: &'a TransferRequest) -> FutureResult<'a, ()>;
    fn get<'a>(&'a self, request: &'a TransferRequest) -> FutureResult<'a, EncodedBlock>;
    fn put<'a>(
        &'a self,
        request: &'a TransferRequest,
        representation: Representation,
        encoded: Vec<u8>,
    ) -> FutureResult<'a, PublishedBlock>;
}
pub async fn run<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send>(
    socket: tokio_tungstenite::WebSocketStream<S>,
    attachment: Attachment,
    endpoint: &dyn Endpoint,
    budget: fastctx_sync::admission::ByteBudget,
    frames: fastctx_sync::admission::BytePermit,
) -> Result<(), Error> {
    let bound = &attachment.0;
    let mut stopped = bound.stopped.clone();
    if *stopped.borrow() || stopped.has_changed().is_err() {
        return Err(Error::Unavailable("asset control session ended".into()));
    }
    tokio::select! {
        _ = stopped.changed() => Err(Error::Unavailable("asset control session ended".into())),
        result = async {
            let mut channel = Channel::new(socket, bound.selected.clone(), bound.ready.session.clone(), budget, frames).await?;
            loop {
                endpoint.current()?;
                let request = match channel.next().await? {
                    Event::MetadataCredit => continue,
                    Event::Metadata(done) => {
                        let prepared = (|| {
                            let request = fastctx_sync::transport_flow::decode_message(done.bytes(), &channel.budget)?;
                            let response = channel.budget.try_acquire(channel.selected.limits().max_metadata_bytes as usize * 8)?;
                            Ok::<_, Error>((request, response))
                        })();
                        let (request, _response) = match prepared {
                            Ok(prepared) => prepared,
                            Err(error) => {
                                channel.control(error_reply(&channel.session, None, &error)).await?;
                                continue;
                            }
                        };
                        endpoint.current()?;
                        let result = super::answers::answer(endpoint, request.value(), channel.selected.limits().max_metadata_bytes).await;
                        match result {
                            Ok(answer) => {
                                if let Err(error) = channel.send_message_checked(done.message_id.clone(), &answer, Some((endpoint, request.value()))).await {
                                    let error = super::answers::failed_delivery(request.value(), error);
                                    channel.control(error_reply(&channel.session, None, &error)).await?;
                                    return Err(error);
                                }
                            }
                            Err(error) => channel.control(error_reply(&channel.session, None, &error)).await?,
                        }
                        drop(request);
                        continue;
                    }
                    Event::Control(TransportControl::TransferRequest(request)) => request,
                    Event::Control(TransportControl::SessionClose(_)) => return Ok(()),
                    _ => return Err(Error::Invalid("expected asset metadata or a transfer request".into())),
                };
                let result = async {
                    request.validate().map_err(Error::Invalid)?;
                    if !bound.binding.contains(&request) { return Err(Error::Denied("transfer exceeds its session boundary".into())); }
                    if !matches!(request.target, TransferTarget::Single { .. }) { return Err(Error::Unavailable("manifest range transfer is not installed".into())); }
                    endpoint.current()?;
                    endpoint.authorize_transfer(&request).await?;
                    let mut permit = parts::memory(&channel)?;
                    match request.direction {
                        TransferDirection::Upload => {
                            let mut received = parts::receive(&mut channel, &request, permit).await?;
                            received._permit.shrink_to(received.bytes.capacity())?;
                            endpoint.current()?;
                            let part = received.offer.part;
                            let stored = endpoint.put(&request, received.offer.representation, received.bytes).await?;
                            let ack = DurableAck::Block { holding: stored.pin.holding, object: stored.pin.object, kind: stored.pin.kind, schema: request.kind.schema, content: stored.content };
                            channel.control(TransportControl::PartCommitted(PartCommitted { session: channel.session.clone(), transfer: request.transfer.clone(), part, ack, stored_representation: stored.representation })).await?;
                        }
                        TransferDirection::Download => {
                            let block = endpoint.get(&request).await?;
                            endpoint.current()?;
                            let part = PartId::from_bytes(crate::sign::random::<16>().map_err(super::io_error)?);
                            parts::send(&mut channel, &request, block.representation, &block.bytes, part, &mut permit, Some(endpoint)).await?;

                        }
                    }
                    channel.control(TransportControl::TransferFinished(TransferFinished { session: channel.session.clone(), transfer: request.transfer.clone() })).await
                }.await;
                if let Err(error) = result {
                    channel.control(error_reply(&channel.session, Some(request.transfer), &error)).await?;
                    // Fragments already sent by the peer cannot become the next
                    // request after a refused or interrupted transfer.
                    return Err(error);
                }
            }
        } => result,
    }
}
fn error_reply(session: &SessionId, transfer: Option<TransferId>, error: &Error) -> TransportControl {
    let (code, detail) = match error {
        Error::Busy => (TransferErrorCode::Busy, "asset resources are occupied"),
        Error::Full => (TransferErrorCode::DiskFull, "asset storage is full"),
        Error::Denied(_) => (TransferErrorCode::NotAccessible, "asset request is not accessible"),
        Error::Revoked => (TransferErrorCode::Revoked, "asset holding was revoked"),
        Error::Invalid(_) => (TransferErrorCode::InvalidRequest, "asset request is invalid"),
        Error::Corrupt(_) => (TransferErrorCode::CorruptContent, "asset content failed verification"),
        Error::OutcomeUnknown { .. } => {
            (TransferErrorCode::OutcomeUnknown, "asset outcome must be queried by its operation")
        }
        _ => (TransferErrorCode::SourceUnavailable, "asset service is unavailable"),
    };
    TransportControl::Error(TransferError {
        session: session.clone(),
        transfer,
        part: None,
        code,
        retry_after_ms: (code == TransferErrorCode::Busy).then_some(1000),
        detail: detail.into(),
    })
}
