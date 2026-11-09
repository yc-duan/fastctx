//! Frozen framing and flow-control examples; no storage or service is started.
use fastctx_types::sync::{
    cursor::{BlockRange, ByteCount, Count, DurableAck, HoldingPeriod, Sequence},
    identity::{Digest, ObjectId, ObjectKey, OperationId},
    manifest::{BlobId, ContentBlock, DigestAlgorithm, Encoding, Envelope, ManifestRoot, Protection, Representation},
    session::*,
    transfer::{BlockFrame, KindVersion, Message},
    transport_codec::{self as codec, CodecError, Frame},
    transport_flow::{CreditWindow, FlowError, MetadataBudget, MetadataReassembly},
};
use serde_json::{Value, json};

pub fn sample(holding: &HoldingPeriod) -> Result<Value, Box<dyn std::error::Error>> {
    let session = SessionId::from_bytes([41; 16]);
    let transfer = TransferId::from_bytes([42; 16]);
    let part = PartId::from_bytes([43; 16]);
    let operation = OperationId::from_bytes([44; 32]);
    let object = ObjectKey { scope: holding.scope.clone(), object: ObjectId::from_bytes([45; 32]) };
    let kind = KindVersion { kind: "conversation.image".into(), schema: 1 };
    let limits = TransportLimits {
        max_frame_bytes: 65536,
        max_control_bytes: 16384,
        max_metadata_bytes: 262144,
        max_active_transfers: 4,
        max_active_parts: 2,
    };
    let payload = b"wire-v1";
    let content = ContentBlock {
        id: BlobId { domain: holding.scope.clone(), algorithm: DigestAlgorithm::Sha256, digest: Digest::of(payload) },
        decoded_bytes: payload.len() as u32,
    };
    let representation = Representation {
        content: content.clone(),
        encoded_bytes: payload.len() as u32,
        encoded_digest: Digest::of(payload),
        envelope: Envelope { version: 1, encoding: Encoding::Raw, protection: Protection::Plain },
    };
    let request = TransferRequest {
        session: session.clone(),
        transfer: transfer.clone(),
        operation: operation.clone(),
        direction: TransferDirection::Download,
        object: object.clone(),
        kind: kind.clone(),
        holding: holding.clone(),
        target: TransferTarget::Single { content: content.clone() },
        representation: RepresentationChoice::Encodings { preferred: vec![Encoding::Raw, Encoding::Zstd] },
        priority: TransferPriority::Interactive,
    };
    let ack = DurableAck::Block {
        holding: holding.clone(),
        object: object.clone(),
        kind: kind.kind.clone(),
        schema: 1,
        content: content.clone(),
    };
    let open = SessionOpen {
        session: session.clone(),
        binding: SessionBindingRequest::Object { object: object.clone(), kind: kind.clone() },
        limits: limits.clone(),
    };
    let ready = SessionReady { session: session.clone(), holding: holding.clone(), limits: limits.clone() };
    let close = SessionClose { session: session.clone() };
    let mut controls = vec![
        TransportControl::SessionOpen(open.clone()),
        TransportControl::SessionOpen(SessionOpen {
            binding: SessionBindingRequest::Scope { scope: holding.scope.clone() },
            ..open.clone()
        }),
        TransportControl::SessionReady(ready.clone()),
        TransportControl::TransferRequest(request.clone()),
        TransportControl::TransferRequest(TransferRequest { direction: TransferDirection::Upload, ..request.clone() }),
        TransportControl::TransferRequest(TransferRequest {
            representation: RepresentationChoice::Exact {
                content: content.id.clone(),
                representation: representation.id(),
            },
            ..request.clone()
        }),
        TransportControl::TransferRequest(TransferRequest {
            target: TransferTarget::ManifestRanges {
                root: ManifestRoot(content.id.clone()),
                ranges: vec![
                    BlockRange { start: Count(0), end: Count(2) },
                    BlockRange { start: Count(7), end: Count(8) },
                ],
            },
            priority: TransferPriority::Background,
            ..request.clone()
        }),
        TransportControl::PartOffer(PartOffer {
            session: session.clone(),
            transfer: transfer.clone(),
            part: part.clone(),
            block_index: None,
            representation: representation.clone(),
        }),
        TransportControl::PartReady(PartReady {
            session: session.clone(),
            transfer: transfer.clone(),
            part: part.clone(),
            durable_offset: ByteCount(3),
        }),
        TransportControl::Credit(Credit {
            session: session.clone(),
            stream: CreditStream::Transfer { transfer: transfer.clone() },
            allowed_payload_total: ByteCount(65536),
        }),
        TransportControl::Credit(Credit {
            session: session.clone(),
            stream: CreditStream::Metadata,
            allowed_payload_total: ByteCount(4096),
        }),
        TransportControl::Stored(Stored {
            session: session.clone(),
            transfer: transfer.clone(),
            part: part.clone(),
            durable_offset: ByteCount(3),
        }),
        TransportControl::PartEnd(PartEnd { session: session.clone(), transfer: transfer.clone(), part: part.clone() }),
        TransportControl::PartCommitted(PartCommitted {
            session: session.clone(),
            transfer: transfer.clone(),
            part: part.clone(),
            ack,
            stored_representation: representation.id(),
        }),
        TransportControl::TransferFinished(TransferFinished { session: session.clone(), transfer: transfer.clone() }),
        TransportControl::TransferCancel(TransferCancel {
            session: session.clone(),
            transfer: transfer.clone(),
            operation: operation.clone(),
        }),
        TransportControl::TransferCancelled(TransferCancelled {
            session: session.clone(),
            transfer: transfer.clone(),
            operation,
        }),
        TransportControl::SessionClose(close.clone()),
    ];
    let codes = [
        TransferErrorCode::UnsupportedProtocol,
        TransferErrorCode::UnsupportedEncoding,
        TransferErrorCode::InvalidRequest,
        TransferErrorCode::TooLarge,
        TransferErrorCode::NotAccessible,
        TransferErrorCode::StaleHolding,
        TransferErrorCode::Revoked,
        TransferErrorCode::StaleSession,
        TransferErrorCode::TransferConflict,
        TransferErrorCode::OffsetMismatch,
        TransferErrorCode::RepresentationChanged,
        TransferErrorCode::CorruptFrame,
        TransferErrorCode::CorruptRepresentation,
        TransferErrorCode::CorruptContent,
        TransferErrorCode::Busy,
        TransferErrorCode::DiskFull,
        TransferErrorCode::SourceUnavailable,
        TransferErrorCode::Cancelled,
        TransferErrorCode::OutcomeUnknown,
    ];
    for code in codes {
        controls.push(TransportControl::Error(TransferError {
            session: session.clone(),
            transfer: Some(transfer.clone()),
            part: Some(part.clone()),
            code,
            retry_after_ms: None,
            detail: String::new(),
        }));
    }
    for control in &controls {
        let bytes = codec::encode(&Frame::Control(control.clone()), &limits)?;
        assert_eq!(codec::decode(&bytes, &limits)?, Frame::Control(control.clone()));
    }
    let header = DataHeader {
        session: session.clone(),
        transfer: transfer.clone(),
        part,
        frame: Sequence(1),
        block: BlockFrame {
            holding: holding.clone(),
            blob: content.id.clone(),
            representation: representation.id(),
            offset: ByteCount(0),
            payload_bytes: payload.len() as u32,
            frame_digest: Digest::of(payload),
        },
    };
    let control_bytes = codec::encode(&Frame::Control(controls[10].clone()), &limits)?;
    let data_bytes = codec::encode(&Frame::BlockData { header: header.clone(), payload }, &limits)?;
    let metadata_header = MetadataPart {
        session: session.clone(),
        message_id: MessageId::from_bytes([46; 16]),
        offset: ByteCount(0),
        total_bytes: ByteCount(2),
    };
    let metadata_bytes = codec::encode(&Frame::MetadataPart { header: metadata_header, payload: b"{}" }, &limits)?;
    let envelopes = [(&control_bytes, "control"), (&data_bytes, "block_data"), (&metadata_bytes, "metadata_part")]
        .into_iter()
        .map(|(bytes, name)| json!({"name":name,"hex":hex(bytes)}))
        .collect::<Vec<_>>();
    let mut unknown = data_bytes.clone();
    unknown[5] = 255;
    assert!(matches!(codec::decode(&unknown, &limits), Err(CodecError::UnknownTag(255))));
    let mut long = data_bytes.clone();
    long[12..16].copy_from_slice(&65536u32.to_be_bytes());
    assert!(matches!(codec::decode(&long, &limits), Err(CodecError::TooLarge)));
    let short = &data_bytes[..data_bytes.len() - 1];
    assert!(matches!(codec::decode(short, &limits), Err(CodecError::LengthMismatch)));
    let mut corrupt = data_bytes.clone();
    *corrupt.last_mut().unwrap() ^= 1;
    assert!(matches!(codec::decode(&corrupt, &limits), Err(CodecError::CorruptFrame)));

    let mut sender = CreditWindow::new(session.clone(), CreditStream::Transfer { transfer: transfer.clone() });
    let mut receiver = CreditWindow::new(session.clone(), CreditStream::Transfer { transfer: transfer.clone() });
    let first = receiver.grant(4096)?;
    sender.apply(&first)?;
    let mut count = 0;
    let mut sent = 0;
    while sent < fastctx_types::sync::BLOCK_BYTES {
        let n = sender.next_payload(60 * 1024).min(fastctx_types::sync::BLOCK_BYTES - sent);
        assert!(n > 0 && n <= 4096);
        sender.consume(n)?;
        receiver.consume(n)?;
        sender.apply(&first)?;
        assert_eq!(sender.available(), ByteCount(0));
        sender.apply(&receiver.grant(n)?)?;
        sent += n;
        count += 1;
    }
    let wrong = Credit { session: SessionId::from_bytes([47; 16]), ..first };
    assert_eq!(sender.apply(&wrong), Err(FlowError::WrongStream));
    assert_eq!(sender.consume(4097), Err(FlowError::NoCredit));
    let before = sender.available();
    sender.apply(&Credit {
        session: session.clone(),
        stream: CreditStream::Transfer { transfer: transfer.clone() },
        allowed_payload_total: ByteCount(1),
    })?;
    assert_eq!(sender.available(), before);

    let budget = MetadataBudget::new(8 * 8192);
    let metadata_limits = TransportLimits { max_metadata_bytes: 8192, ..limits.clone() };
    let mut assembly = MetadataReassembly::new(session.clone(), metadata_limits.clone(), budget.clone())?;
    let other_session = SessionId::from_bytes([50; 16]);
    let mut other = MetadataReassembly::new(other_session.clone(), metadata_limits, budget.clone())?;
    let blocked = MetadataPart {
        session: session.clone(),
        message_id: MessageId::from_bytes([51; 16]),
        offset: ByteCount(0),
        total_bytes: ByteCount(8192),
    };
    let mut meta_sender = CreditWindow::new(session.clone(), CreditStream::Metadata);
    let mut meta_receiver = CreditWindow::new(session.clone(), CreditStream::Metadata);
    meta_sender.apply(&meta_receiver.grant(1024)?)?;
    let mut complete = None;
    for offset in (0..8192).step_by(1024) {
        let part = MetadataPart {
            session: session.clone(),
            message_id: MessageId::from_bytes([48; 16]),
            offset: ByteCount(offset),
            total_bytes: ByteCount(8192),
        };
        let payload = vec![b' '; 1024];
        meta_sender.consume(1024)?;
        meta_receiver.consume(1024)?;
        complete = assembly.push(&part, &payload)?;
        meta_sender.apply(&meta_receiver.grant(1024)?)?;
        assert_eq!(budget.used(), 8192);
        assert!(matches!(assembly.push(&blocked, &[0]), Err(CodecError::Busy)));
        let independent = MetadataPart { session: other_session.clone(), ..blocked.clone() };
        assert!(other.push(&independent, &[0])?.is_none());
        assert_eq!(budget.used(), 2 * 8192);
        other.cancel(&independent.message_id);
        assert_eq!(budget.used(), 8192);
    }
    assert_eq!(complete.as_ref().unwrap().bytes().len(), 8192);
    assert_eq!(budget.used(), 8192);
    drop(assembly);
    assert_eq!(budget.used(), 8192);
    drop(complete);
    assert_eq!(budget.used(), 0);

    let mut reconnected = request.clone();
    reconnected.session = SessionId::from_bytes([49; 16]);
    assert_eq!(request.fingerprint()?, reconnected.fingerprint()?);
    reconnected.holding.publication_epoch.0 += 1;
    assert_ne!(request.fingerprint()?, reconnected.fingerprint()?);
    Ok(json!({
        "protocol":"asset-transfer/v1", "magic_hex":"46585354", "prefix_bytes":16,
        "tags":{"transport_control":1,"block_data":2,"metadata_part":3},
        "limits":limits, "control_plane":[Message::OpenSyncSession { request:open },Message::SessionReady { ready },Message::SessionClose { close }],
        "controls":controls,"data_header":header,"envelopes":envelopes,
        "negative_envelopes":[
            {"name":"unknown_tag","hex":hex(&unknown),"error":"unknown_tag"},
            {"name":"whole_frame_too_long","hex":hex(&long),"error":"too_large"},
            {"name":"length_mismatch","hex":hex(short),"error":"length_mismatch"},
            {"name":"payload_digest_mismatch","hex":hex(&corrupt),"error":"corrupt_frame"}
        ],
        "flow_contract":{
            "block_bytes":sent,"window_bytes":4096,"consume_and_return_rounds":count,
            "durable_ack_from_credit":false,"duplicate_and_older_credit_adds_bytes":false,
            "cross_session_credit":wrong,"cross_session_result":"wrong_stream",
            "metadata_bytes":8192,"metadata_window_bytes":1024,"completed_buffer_still_reserved":true,
            "metadata_session_fraction":"1/8","stalled_session_blocks_other_session":false,
            "queued_completion_keeps_session_reservation":true,
            "request_digest_survives_reconnect":true,"holding_change_changes_request_digest":true
        }
    }))
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
