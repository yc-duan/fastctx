//! Wire adapters for a separately authenticated asset data WebSocket.
//!
//! This module does not advertise a capability or install a listener. Service
//! composition must bind the authenticated control connection, TLS peer, hub,
//! incarnation and current holding before attaching this socket. Neither this
//! codec nor an old Hands/Data ticket grants read or retention permission.

use fastctx_types::sync::{
    session::{SessionId, TRANSPORT_PROTOCOL, TransportLimits},
    transfer::{Capabilities, SelectedEnvelope, SelectedProtocol, SelectedTransport, Selection},
    transport_codec::{self, CodecError, Frame},
};
use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{Message, protocol::WebSocketConfig},
};

/// Selection proves compatible framing, not authorization or service readiness.
/// Create it only from capabilities obtained through the legacy-readable
/// authenticated handshake; never send a new Hello merely to probe an old peer.
#[derive(Clone, Debug)]
pub struct NegotiatedTransport {
    limits: TransportLimits,
    selection: Selection,
}
impl NegotiatedTransport {
    pub fn select(
        local: &Capabilities,
        remote: &Capabilities,
        local_limits: &TransportLimits,
        remote_limits: &TransportLimits,
    ) -> Result<Self, String> {
        for capabilities in [local, remote] {
            if !capabilities.protocols.iter().any(|p| p == "fastctx-sync/v1")
                || !capabilities.envelopes.iter().any(|p| p == "plain/v1")
                || !capabilities.transports.iter().any(|p| p == TRANSPORT_PROTOCOL)
                || capabilities.max_page_bytes == 0
                || capabilities.receive_window.0 == 0
            {
                return Err("asset transport was not mutually advertised".into());
            }
        }
        let encoding = [fastctx_types::sync::manifest::Encoding::Raw, fastctx_types::sync::manifest::Encoding::Zstd]
            .into_iter()
            .find(|encoding| {
                let name = match encoding {
                    fastctx_types::sync::manifest::Encoding::Raw => "raw",
                    _ => "zstd",
                };
                local.encodings.iter().any(|e| e == name) && remote.encodings.iter().any(|e| e == name)
            })
            .ok_or_else(|| "no mutually advertised asset encoding".to_string())?;
        let mut limits = local_limits.intersect(remote_limits)?;
        limits.max_metadata_bytes = limits.max_metadata_bytes.min(local.max_page_bytes).min(remote.max_page_bytes);
        Ok(Self {
            limits,
            selection: Selection {
                protocol: SelectedProtocol::V1,
                envelope: SelectedEnvelope::V1,
                encoding,
                transport: Some(SelectedTransport::V1),
            },
        })
    }
    pub fn limits(&self) -> &TransportLimits {
        &self.limits
    }
    pub fn selection(&self) -> &Selection {
        &self.selection
    }
    /// Bounds WebSocket fragmentation before the library aggregates a message.
    /// Install at the WebSocket handshake, on both accepted and dialed sockets.
    pub fn websocket_config(&self) -> WebSocketConfig {
        let bound = self.limits.max_frame_bytes as usize;
        crate::dial::config(bound).read_buffer_size(bound).write_buffer_size(0).max_write_buffer_size(bound + 128)
    }
    /// A caller supplies the session already bound on its authenticated control
    /// connection. The receiving service must verify the existing TLS exporter
    /// proof and compare device/hub/incarnation, not merely look up this ID.
    pub fn hub_intent(
        &self,
        session: SessionId,
        key: crate::sign::PublicKey,
        proof: crate::sign::Signature,
    ) -> crate::wire::Intent {
        crate::wire::Intent::AssetSyncData { session, key, proof }
    }
    pub fn peer_intent(
        &self,
        session: SessionId,
        key: crate::sign::PublicKey,
        proof: crate::sign::Signature,
    ) -> crate::peer::Intent {
        crate::peer::Intent::AssetSyncData { session, key, proof }
    }
}

/// Bounded bytes retained once; decoding borrows the payload in this buffer.
/// The caller must charge its frame budget until this value is dropped.
pub struct ReceivedFrame {
    bytes: bytes::Bytes,
    limits: TransportLimits,
}
impl ReceivedFrame {
    pub fn decode(&self) -> Result<Frame<'_>, CodecError> {
        transport_codec::decode(&self.bytes, &self.limits)
    }
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error(transparent)]
    Codec(#[from] CodecError),
    #[error("asset WebSocket failed: {0}")]
    Socket(#[from] tokio_tungstenite::tungstenite::Error),
    #[error("asset channel requires binary messages")]
    NotBinary,
    #[error("asset WebSocket must be bounded at its handshake")]
    UnboundedSocket,
}

/// Receives one frame without accumulating a transfer or a storage block.
/// Credit, authorization and persistence are enforced by the bound session
/// runtime before its next read/send; this adapter does not generate any ACK.
pub async fn recv<S>(
    socket: &mut WebSocketStream<S>,
    selected: &NegotiatedTransport,
) -> Result<Option<ReceivedFrame>, TransportError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    check_config(socket, selected)?;
    loop {
        match socket.next().await {
            None | Some(Ok(Message::Close(_))) => return Ok(None),
            Some(Ok(Message::Binary(bytes))) => {
                transport_codec::decode(&bytes, selected.limits())?;
                return Ok(Some(ReceivedFrame { bytes, limits: selected.limits.clone() }));
            }
            Some(Ok(Message::Ping(_) | Message::Pong(_))) => socket.flush().await?,
            Some(Ok(Message::Text(_) | Message::Frame(_))) => return Err(TransportError::NotBinary),
            Some(Err(error)) => return Err(error.into()),
        }
    }
}
pub async fn send<S>(
    socket: &mut WebSocketStream<S>,
    selected: &NegotiatedTransport,
    frame: &Frame<'_>,
) -> Result<(), TransportError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    check_config(socket, selected)?;
    let bytes = transport_codec::encode(frame, selected.limits())?;
    socket.send(Message::Binary(bytes.into())).await?;
    Ok(())
}

fn check_config<S>(socket: &WebSocketStream<S>, selected: &NegotiatedTransport) -> Result<(), TransportError> {
    let config = socket.get_config();
    let bound = selected.limits.max_frame_bytes as usize;
    if config.max_message_size.is_none_or(|bytes| bytes > bound)
        || config.max_frame_size.is_none_or(|bytes| bytes > bound)
        || config.read_buffer_size > bound
        || config.max_write_buffer_size > bound + 128
    {
        return Err(TransportError::UnboundedSocket);
    }
    Ok(())
}
