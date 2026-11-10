//! A data connection owns its frame reservation and metadata assembly until drop.
use super::{io_error, remote_error};
use crate::{
    sync_transport::{self, NegotiatedTransport},
    transport::Transport,
};
use fastctx_sync::{
    admission::{ByteBudget, BytePermit},
    store::Error,
    transport_flow::{CompletedMetadata, MetadataReassembly},
};
use fastctx_types::sync::{
    cursor::ByteCount,
    session::{CreditStream, MessageId, MetadataPart, SessionId, TransportControl},
    transfer::Message,
    transport_codec::{self, Frame},
    transport_flow::CreditWindow,
};
use std::{
    io::Write,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::time::Instant;

/// Reserve before constructing the WebSocket, whose buffers allocate at setup.
pub fn frame_bytes(selected: &NegotiatedTransport) -> usize {
    selected.limits().max_frame_bytes as usize * 4
}

pub enum Event {
    MetadataCredit,
    Metadata(CompletedMetadata),
    Control(TransportControl),
    Data { header: fastctx_types::sync::session::DataHeader, bytes: Vec<u8> },
}
/// A caller serializes request/reply work on this socket. Cancellation drops it;
/// no subsequent request may consume the abandoned request's response.
pub struct Channel<S = tokio_rustls::TlsStream<Transport>> {
    socket: tokio_tungstenite::WebSocketStream<S>,
    pub selected: NegotiatedTransport,
    pub session: SessionId,
    pub budget: ByteBudget,
    pub(crate) attempted: Arc<AtomicBool>,
    pub(crate) refused: Arc<AtomicBool>,
    _frames: BytePermit,
    assembly: MetadataReassembly,
    sending: CreditWindow,
    receiving: CreditWindow,
    progress: Instant,
}
impl<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send> Channel<S> {
    pub async fn new(
        socket: tokio_tungstenite::WebSocketStream<S>,
        selected: NegotiatedTransport,
        session: SessionId,
        budget: ByteBudget,
        mut frames: BytePermit,
    ) -> Result<Self, Error> {
        frames.shrink_to(frame_bytes(&selected))?;
        let assembly =
            MetadataReassembly::new(session.clone(), selected.limits().clone(), budget.clone()).map_err(io_error)?;
        let mut channel = Self {
            attempted: Arc::new(AtomicBool::new(false)),
            refused: Arc::new(AtomicBool::new(false)),
            socket,
            selected,
            session: session.clone(),
            budget,
            _frames: frames,
            assembly,
            sending: CreditWindow::new(session.clone(), CreditStream::Metadata),
            receiving: CreditWindow::new(session, CreditStream::Metadata),
            progress: Instant::now(),
        };
        let credit = channel.receiving.grant(channel.selected.limits().max_frame_bytes).map_err(io_error)?;
        channel.control(TransportControl::Credit(credit)).await?;
        Ok(channel)
    }
    pub async fn control(&mut self, value: TransportControl) -> Result<(), Error> {
        if value.session() != &self.session {
            return Err(Error::Invalid("control belongs to another asset session".into()));
        }
        let progress = !matches!(value, TransportControl::Credit(_));
        if matches!(value, TransportControl::TransferRequest(_)) {
            self.attempted.store(true, Ordering::Release);
        }
        tokio::time::timeout(
            crate::wire::SILENCE,
            sync_transport::send(&mut self.socket, &self.selected, &Frame::Control(value)),
        )
        .await
        .map_err(|_| Error::Unavailable("asset control send was silent".into()))?
        .map_err(io_error)?;
        if progress {
            self.progress = Instant::now();
        }
        Ok(())
    }
    pub async fn frame(&mut self, frame: &Frame<'_>) -> Result<(), Error> {
        if frame.session() != &self.session {
            return Err(Error::Invalid("frame belongs to another asset session".into()));
        }
        self.attempted.store(true, Ordering::Release);
        tokio::time::timeout(crate::wire::SILENCE, sync_transport::send(&mut self.socket, &self.selected, frame))
            .await
            .map_err(|_| Error::Unavailable("asset frame send was silent".into()))?
            .map_err(io_error)?;
        self.progress = Instant::now();
        Ok(())
    }
    pub async fn next(&mut self) -> Result<Event, Error> {
        // Progress, not total transfer duration, bounds a slow peer. Replayed
        // credit cannot keep a socket alive without moving any work forward.
        loop {
            let bytes = tokio::time::timeout_at(
                self.progress + crate::wire::SILENCE,
                sync_transport::recv(&mut self.socket, &self.selected),
            )
            .await
            .map_err(|_| Error::Unavailable("asset data connection was silent".into()))?
            .map_err(io_error)?
            .ok_or_else(|| Error::Unavailable("asset data connection ended".into()))?;
            let frame = bytes.decode().map_err(io_error)?;
            if frame.session() != &self.session {
                return Err(Error::Invalid("frame belongs to another asset session".into()));
            }
            match frame {
                Frame::Control(TransportControl::Credit(credit)) if credit.stream == CreditStream::Metadata => {
                    self.sending.apply(&credit).map_err(io_error)?;
                    return Ok(Event::MetadataCredit);
                }
                Frame::Control(TransportControl::Error(error)) => {
                    self.refused.store(
                        error.code != fastctx_types::sync::session::TransferErrorCode::OutcomeUnknown,
                        Ordering::Release,
                    );
                    return Err(remote_error(error));
                }
                Frame::Control(value) => {
                    if !matches!(value, TransportControl::Credit(_)) {
                        self.progress = Instant::now();
                    }
                    return Ok(Event::Control(value));
                }
                Frame::MetadataPart { header, payload } => {
                    self.receiving.consume(payload.len() as u32).map_err(io_error)?;
                    let done = self.assembly.push(&header, payload).map_err(io_error)?;
                    self.progress = Instant::now();
                    let credit = self.receiving.grant(payload.len() as u32).map_err(io_error)?;
                    self.control(TransportControl::Credit(credit)).await?;
                    if let Some(done) = done {
                        return Ok(Event::Metadata(done));
                    }
                }
                Frame::BlockData { header, payload } => {
                    self.progress = Instant::now();
                    return Ok(Event::Data { header, bytes: payload.to_vec() });
                }
            }
        }
    }
    pub async fn send_message(&mut self, id: MessageId, message: &Message) -> Result<(), Error> {
        self.send_message_checked(id, message, None).await
    }
    pub(crate) async fn send_message_checked(
        &mut self,
        id: MessageId,
        message: &Message,
        check: Option<(&dyn super::server::Endpoint, &Message)>,
    ) -> Result<(), Error> {
        let bound = self.selected.limits().max_metadata_bytes as usize;
        let _permit = self.budget.try_acquire(bound)?;
        let mut out = Bounded { bytes: Vec::with_capacity(bound), limit: bound };
        serde_json::to_writer(&mut out, message)
            .map_err(|_| Error::Invalid("asset message exceeds its negotiated byte bound".into()))?;
        let mut offset = 0;
        while offset < out.bytes.len() {
            let header = MetadataPart {
                session: self.session.clone(),
                message_id: id.clone(),
                offset: ByteCount(offset as u64),
                total_bytes: ByteCount(out.bytes.len() as u64),
            };
            let capacity = transport_codec::payload_capacity(&header, self.selected.limits()).map_err(io_error)?;
            let n = self.sending.next_payload(capacity).min((out.bytes.len() - offset) as u32) as usize;
            if n == 0 {
                // In request/reply order, only the receiver's returned credit can
                // arrive before it has received this whole message.
                match self.next().await? {
                    Event::MetadataCredit => continue,
                    Event::Control(_) | Event::Data { .. } | Event::Metadata(_) => {
                        return Err(Error::Invalid("peer sent work while a metadata message was incomplete".into()));
                    }
                }
            }
            if let Some((endpoint, request)) = check {
                endpoint.current()?;
                endpoint.recheck_message(request).await.map_err(|error| match error {
                    Error::Denied(_) | Error::Revoked => Error::Revoked,
                    other => other,
                })?;
            }
            self.sending.consume(n as u32).map_err(io_error)?;
            self.frame(&Frame::MetadataPart { header, payload: &out.bytes[offset..offset + n] }).await?;
            offset += n;
        }
        Ok(())
    }
    pub async fn receive_message(&mut self) -> Result<(MessageId, fastctx_sync::replicate::Received<Message>), Error> {
        let done = loop {
            match self.next().await? {
                Event::MetadataCredit => continue,
                Event::Metadata(done) => break done,
                _ => return Err(Error::Invalid("expected an asset metadata message".into())),
            }
        };
        let message = fastctx_sync::transport_flow::decode_message(done.bytes(), &self.budget)?;
        Ok((done.message_id.clone(), message))
    }
}
struct Bounded {
    bytes: Vec<u8>,
    limit: usize,
}
impl Write for Bounded {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.bytes.len().saturating_add(bytes.len()) > self.limit {
            return Err(std::io::Error::other("metadata byte limit"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
