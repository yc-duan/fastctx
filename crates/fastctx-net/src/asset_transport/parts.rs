//! A bounded single block is acknowledged only after the receiver publishes it.
use super::{
    channel::{Channel, Event},
    io_error,
};
use fastctx_sync::{admission::BytePermit, blocks::codec, store::Error};
use fastctx_types::sync::{
    cursor::{ByteCount, Sequence},
    identity::Digest,
    manifest::Representation,
    session::*,
    transfer::BlockFrame,
    transport_codec::{self, Frame},
    transport_flow::CreditWindow,
};

pub struct ReceivedPart {
    pub offer: PartOffer,
    pub bytes: Vec<u8>,
    pub _permit: BytePermit,
}
pub(crate) const WORKING_BYTES: usize = (fastctx_types::sync::BLOCK_BYTES as usize + 65536) * 2 + 2 * 1024 * 1024;
pub fn memory<S>(channel: &Channel<S>) -> Result<BytePermit, Error> {
    channel.budget.try_acquire(WORKING_BYTES)
}
fn matches(request: &TransferRequest, representation: &Representation) -> Result<(), Error> {
    let TransferTarget::Single { content } = &request.target else {
        return Err(Error::Unavailable("manifest range transfer is not installed".into()));
    };
    if content != &representation.content {
        return Err(Error::Invalid("part differs from the requested content".into()));
    }
    representation.validate().map_err(Error::Invalid)?;
    match &request.representation {
        RepresentationChoice::Encodings { preferred } if preferred.contains(&representation.envelope.encoding) => {
            Ok(())
        }
        RepresentationChoice::Exact { representation: id, .. } if id == &representation.id() => Ok(()),
        _ => Err(Error::Invalid("part uses an unrequested representation".into())),
    }
}
pub async fn receive<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send>(
    channel: &mut Channel<S>,
    request: &TransferRequest,
    permit: BytePermit,
) -> Result<ReceivedPart, Error> {
    // Full admission and authorization precede this credit. A fresh part always
    // starts at zero, so no separate ready round trip is needed.
    let mut credit =
        CreditWindow::new(channel.session.clone(), CreditStream::Transfer { transfer: request.transfer.clone() });
    let grant = credit
        .grant((fastctx_types::sync::BLOCK_BYTES + 65536).min(channel.selected.receive_window()))
        .map_err(io_error)?;
    channel.control(TransportControl::Credit(grant)).await?;
    let offer = loop {
        match channel.next().await? {
            Event::MetadataCredit => continue,
            Event::Control(TransportControl::PartOffer(offer))
                if offer.transfer == request.transfer && offer.block_index.is_none() =>
            {
                break offer;
            }
            _ => return Err(Error::Invalid("expected this transfer's part offer".into())),
        }
    };
    matches(request, &offer.representation)?;
    if !channel.selected.supports_encoding(offer.representation.envelope.encoding) {
        return Err(Error::Invalid("part encoding was not negotiated".into()));
    }
    let mut bytes = Vec::with_capacity(offer.representation.encoded_bytes as usize);
    let mut sequence = 0;
    loop {
        match channel.next().await? {
            Event::MetadataCredit => continue,
            Event::Data { header, bytes: payload } => {
                if header.transfer != request.transfer
                    || header.part != offer.part
                    || header.frame.0 != sequence + 1
                    || header.block.holding != request.holding
                    || header.block.blob != offer.representation.content.id
                    || header.block.representation != offer.representation.id()
                    || header.block.offset.0 != bytes.len() as u64
                    || bytes.len() + payload.len() > offer.representation.encoded_bytes as usize
                {
                    return Err(Error::Invalid("part frame differs from its admitted transfer or prefix".into()));
                }
                credit.consume(payload.len() as u32).map_err(io_error)?;
                bytes.extend_from_slice(&payload);
                sequence += 1;
                if credit.available().0 == 0 && bytes.len() < offer.representation.encoded_bytes as usize {
                    let remaining = offer.representation.encoded_bytes - bytes.len() as u32;
                    let returned = credit.grant(remaining.min(channel.selected.receive_window())).map_err(io_error)?;
                    channel.control(TransportControl::Credit(returned)).await?;
                }
            }
            Event::Control(TransportControl::PartEnd(end))
                if end.transfer == request.transfer && end.part == offer.part =>
            {
                codec::validate(&offer.representation, &bytes)?;

                return Ok(ReceivedPart { offer, bytes, _permit: permit });
            }
            _ => return Err(Error::Invalid("unexpected message during a part".into())),
        }
    }
}
pub async fn send<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send>(
    channel: &mut Channel<S>,
    request: &TransferRequest,
    representation: Representation,
    bytes: &[u8],
    part: PartId,
    _permit: &mut BytePermit,
    endpoint: Option<&dyn super::server::Endpoint>,
) -> Result<(), Error> {
    matches(request, &representation)?;
    if !channel.selected.supports_encoding(representation.envelope.encoding) {
        return Err(Error::Invalid("part encoding was not negotiated".into()));
    }
    codec::validate(&representation, bytes)?;
    // The sender borrows a caller-owned allocation whose spare capacity is not
    // visible through the slice. Retain its admitted window through this send.

    channel
        .control(TransportControl::PartOffer(PartOffer {
            session: channel.session.clone(),
            transfer: request.transfer.clone(),
            part: part.clone(),
            block_index: None,
            representation: representation.clone(),
        }))
        .await?;
    let mut credit =
        CreditWindow::new(channel.session.clone(), CreditStream::Transfer { transfer: request.transfer.clone() });
    loop {
        match channel.next().await? {
            Event::MetadataCredit => continue,
            Event::Control(TransportControl::Credit(update)) => {
                credit.apply(&update).map_err(io_error)?;
                break;
            }
            _ => return Err(Error::Invalid("expected this admitted part's byte window".into())),
        }
    }
    let mut offset = 0;
    let mut sequence = 0;
    while offset < bytes.len() {
        let mut header = DataHeader {
            session: channel.session.clone(),
            transfer: request.transfer.clone(),
            part: part.clone(),
            frame: Sequence(sequence + 1),
            block: BlockFrame {
                holding: request.holding.clone(),
                blob: representation.content.id.clone(),
                representation: representation.id(),
                offset: ByteCount(offset as u64),
                payload_bytes: 1,
                frame_digest: Digest::of(&[]),
            },
        };
        let cap = transport_codec::payload_capacity(&header, channel.selected.limits()).map_err(io_error)?;
        let n = credit.next_payload(cap).min((bytes.len() - offset) as u32) as usize;
        if n == 0 {
            match channel.next().await? {
                Event::MetadataCredit => continue,
                Event::Control(TransportControl::Credit(update)) => {
                    credit.apply(&update).map_err(io_error)?;
                    continue;
                }
                _ => return Err(Error::Invalid("expected transfer credit".into())),
            }
        }
        header.block.payload_bytes = n as u32;
        header.block.frame_digest = Digest::of(&bytes[offset..offset + n]);
        // Header width can grow when payload_bytes becomes multi-digit.
        let cap = transport_codec::payload_capacity(&header, channel.selected.limits()).map_err(io_error)? as usize;
        let n = n.min(cap);
        header.block.payload_bytes = n as u32;
        header.block.frame_digest = Digest::of(&bytes[offset..offset + n]);
        if let Some(endpoint) = endpoint {
            let check = async {
                endpoint.current()?;
                endpoint.authorize_transfer(request).await
            };
            check.await.map_err(|error| match error {
                Error::Denied(_) | Error::Revoked => Error::Revoked,
                other => other,
            })?;
        }
        credit.consume(n as u32).map_err(io_error)?;
        channel.frame(&Frame::BlockData { header, payload: &bytes[offset..offset + n] }).await?;
        offset += n;
        sequence += 1;
    }
    channel
        .control(TransportControl::PartEnd(PartEnd {
            session: channel.session.clone(),
            transfer: request.transfer.clone(),
            part,
        }))
        .await
}
