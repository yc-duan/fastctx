//! One admitted request on an already authenticated asset channel.
use super::{Channel, channel::Event, parts};
use fastctx_sync::{
    blocks::codec,
    cursor::DurableAck,
    identity::{Digest, OperationId},
    replicate::{BlockOffer, BlockRequest, Received},
    store::Error,
};
use fastctx_types::sync::{session::*, transfer::Message};

pub async fn exchange<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send>(
    channel: &mut Channel<S>,
    request: Message,
) -> Result<Received<Message>, Error> {
    let id = MessageId::from_bytes(crate::sign::random::<16>().map_err(super::io_error)?);
    channel.send_message(id.clone(), &request).await?;
    let (reply, message) = channel.receive_message().await?;
    if reply != id {
        return Err(Error::Invalid("asset response does not answer this request".into()));
    }
    Ok(message)
}
pub async fn fetch<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send>(
    channel: &mut Channel<S>,
    request: BlockRequest,
) -> Result<Received<Vec<u8>>, Error> {
    let memory = parts::memory(channel)?;
    fetch_prepared(channel, request, memory).await
}
pub(crate) async fn fetch_prepared<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send>(
    channel: &mut Channel<S>,
    request: BlockRequest,
    memory: fastctx_sync::admission::BytePermit,
) -> Result<Received<Vec<u8>>, Error> {
    let operation = OperationId::from_bytes(
        Digest::of(
            &serde_json::to_vec(&(&request.object, &request.kind, &request.block, &request.holding))
                .map_err(|e| Error::Invalid(e.to_string()))?,
        )
        .bytes(),
    );
    let transfer = TransferRequest {
        session: channel.session.clone(),
        transfer: transfer_id(
            &operation,
            &request.holding,
            &request.object,
            &request.kind,
            &request.block,
            &request.root,
            None,
        ),
        operation,
        direction: TransferDirection::Download,
        upload_cause: None,
        object: request.object,
        kind: request.kind,
        holding: request.holding,
        root: Box::new(request.root),
        target: TransferTarget::Single { content: request.block },
        representation: RepresentationChoice::Encodings { preferred: channel.selected.encodings().to_vec() },
        priority: TransferPriority::Background,
    };
    channel.control(TransportControl::TransferRequest(transfer.clone())).await?;
    let received = parts::receive(channel, &transfer, memory).await?;
    let mut decoded_permit = received._permit;
    let decoded = codec::decode(&received.offer.representation, received.bytes)?;
    finished(channel, &transfer).await?;
    decoded_permit.shrink_to(decoded.capacity())?;
    Ok(fastctx_sync::replicate::Received::new(decoded, decoded_permit))
}
pub async fn offer<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send>(
    channel: &mut Channel<S>,
    offer: BlockOffer,
) -> Result<DurableAck, Error> {
    let memory = parts::memory(channel)?;
    offer_prepared(channel, offer, memory).await
}
pub(crate) async fn offer_prepared<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send>(
    channel: &mut Channel<S>,
    offer: BlockOffer,
    mut memory: fastctx_sync::admission::BytePermit,
) -> Result<DurableAck, Error> {
    let transfer = TransferRequest {
        session: channel.session.clone(),
        transfer: transfer_id(
            &offer.operation,
            &offer.holding,
            &offer.object,
            &offer.kind,
            &offer.representation.content,
            &offer.representation.content.clone().into(),
            Some(&offer.cause),
        ),
        operation: offer.operation,
        direction: TransferDirection::Upload,
        upload_cause: Some(offer.cause),
        object: offer.object,
        kind: offer.kind,
        holding: offer.holding,
        root: Box::new(offer.representation.content.clone().into()),
        target: TransferTarget::Single { content: offer.representation.content.clone() },
        representation: RepresentationChoice::Exact {
            content: offer.representation.content.id.clone(),
            representation: offer.representation.id(),
        },
        priority: TransferPriority::Background,
    };
    transfer.validate().map_err(Error::Invalid)?;
    let part = PartId::from_bytes(crate::sign::random::<16>().map_err(crate::asset_transport::io_error)?);
    channel.control(TransportControl::TransferRequest(transfer.clone())).await?;
    parts::send(channel, &transfer, offer.representation.clone(), &offer.encoded, part.clone(), &mut memory, None)
        .await?;
    let ack = loop {
        match channel.next().await? {
            Event::MetadataCredit => continue,
            Event::Control(TransportControl::Credit(update))
                if update.stream == (CreditStream::Transfer { transfer: transfer.transfer.clone() }) =>
            {
                continue;
            }
            Event::Control(TransportControl::PartCommitted(committed))
                if committed.transfer == transfer.transfer && committed.part == part =>
            {
                break committed.ack;
            }
            _ => return Err(Error::Invalid("expected this uploaded part's durable commit".into())),
        }
    };
    let DurableAck::Block { holding, object, kind, schema, content } = &ack else {
        return Err(Error::Invalid("a block upload requires a block ACK".into()));
    };
    if holding.holder != (fastctx_sync::cursor::Holder::Hub { hub: transfer.object.scope.hub.clone() })
        || holding.scope != transfer.object.scope
        || holding.acl_revision != transfer.holding.acl_revision
        || holding.publication_epoch.0 == 0
        || object != &transfer.object
        || kind != &transfer.kind.kind
        || *schema != transfer.kind.schema
        || content != &offer.representation.content
    {
        return Err(Error::Invalid("block ACK differs from the request or actual receiving hub".into()));
    }
    finished(channel, &transfer).await?;
    Ok(ack)
}
fn transfer_id(
    operation: &OperationId,
    holding: &fastctx_sync::cursor::HoldingPeriod,
    object: &fastctx_sync::identity::ObjectKey,
    kind: &fastctx_sync::transfer::KindVersion,
    content: &fastctx_sync::manifest::ContentBlock,
    root: &fastctx_sync::manifest::ContentRef,
    cause: Option<&UploadCause>,
) -> TransferId {
    let mut bytes = b"fastctx.sync.single-transfer/v1".to_vec();
    bytes.push(0);
    serde_json::to_writer(&mut bytes, &(operation, holding, object, kind, content, root, cause))
        .expect("transfer coordinates serialize");
    let hash = Digest::of(&bytes);
    TransferId::from_bytes(hash.bytes()[..16].try_into().expect("fixed transfer identity"))
}
async fn finished<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send>(
    channel: &mut Channel<S>,
    request: &TransferRequest,
) -> Result<(), Error> {
    loop {
        match channel.next().await? {
            Event::MetadataCredit => continue,
            Event::Control(TransportControl::Credit(update))
                if update.stream == (CreditStream::Transfer { transfer: request.transfer.clone() }) =>
            {
                continue;
            }
            Event::Control(TransportControl::TransferFinished(done)) if done.transfer == request.transfer => {
                return Ok(());
            }
            _ => return Err(Error::Invalid("expected completion of this transfer".into())),
        }
    }
}
