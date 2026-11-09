//! What installation and reading need from the database: the installed head,
//! the conversation's stream and records, the original signed batches whose
//! canonical serialization every holder publishes as the same proof block, and
//! the replica custody this holder keeps of the conversation.
use crate::{
    catalog::RecordRef,
    cursor::{HoldingPeriod, LocalSequence, Revision, Sequence, StreamCursor},
    identity::{Digest, ObjectKey, ProducerEpoch, StreamId},
    manifest::{ContentBlock, ManifestRoot},
    proof::{Bytes, ProofPurpose, Signature, SignedSourceBatch, SignedStatement, SigningKey},
    registry::REGISTRY,
    store::{
        access::{AccessContext, Custody, HoldingUse, Principal},
        ingest::StreamProgress,
        roots::RootUse,
        sql,
    },
    timeline::TimelineFence,
};
use fastctx_storage::{
    Result,
    rusqlite::{Connection, OptionalExtension, params},
};
use fastctx_types::sync::conversation_timeline::KIND;

/// What this holder has installed for one conversation.
#[derive(Clone)]
pub(super) struct Head {
    pub projection: Revision,
    pub through: StreamCursor,
    pub root: Option<ManifestRoot>,
    pub indexed: Option<Revision>,
    pub applied: LocalSequence,
    pub local: LocalSequence,
}
/// The conversation's timeline stream as last committed here.
pub(super) struct Stream {
    pub stream: StreamId,
    pub epoch: ProducerEpoch,
    pub sequence: Sequence,
    pub local: LocalSequence,
}
pub(super) struct Record {
    pub reference: RecordRef,
    pub payload: ContentBlock,
    pub proof: Digest,
}

fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    serde_json::from_slice(bytes).map_err(|error| sql::invalid(error.to_string()))
}

pub(super) fn head(db: &Connection, conversation: &ObjectKey) -> Result<Option<Head>> {
    let scope = &conversation.scope;
    let row = db
        .prepare_cached("SELECT projection,through_cursor,root,indexed,applied,local FROM sync_timeline_heads WHERE hub=?1 AND scope=?2 AND conversation=?3")?
        .query_row(params![scope.hub.to_string(), scope.scope.as_str(), conversation.object.as_str()], |row| {
            Ok((
                sql::counter(row, 0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<Vec<u8>>>(3)?,
                sql::counter(row, 4)?,
                sql::counter(row, 5)?,
            ))
        })
        .optional()?;
    row.map(|(projection, through, root, indexed, applied, local)| {
        Ok(Head {
            projection: Revision(projection),
            through: decode(&through)?,
            root: root
                .map(|digest| {
                    Ok::<_, fastctx_storage::Error>(ManifestRoot(crate::manifest::BlobId {
                        domain: scope.clone(),
                        algorithm: crate::manifest::DigestAlgorithm::Sha256,
                        digest: Digest::try_from(digest).map_err(sql::invalid)?,
                    }))
                })
                .transpose()?,
            indexed: indexed
                .map(|bytes| {
                    <[u8; 8]>::try_from(bytes.as_slice())
                        .map(|value| Revision(u64::from_be_bytes(value)))
                        .map_err(|_| sql::invalid("invalid indexed projection"))
                })
                .transpose()?,
            applied: LocalSequence(applied),
            local: LocalSequence(local),
        })
    })
    .transpose()
}

/// The single timeline stream of the conversation. A second epoch means a
/// producer handover, which installation follows only through the hub's path.
pub(super) fn stream(db: &Connection, conversation: &ObjectKey) -> Result<Option<Stream>> {
    let scope = &conversation.scope;
    let rows = db
        .prepare_cached("SELECT stream,epoch,sequence,local FROM sync_stream_heads WHERE hub=?1 AND scope=?2 AND object=?3 AND kind='conversation.timeline' LIMIT 2")?
        .query_map(params![scope.hub.to_string(), scope.scope.as_str(), conversation.object.as_str()], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, sql::counter(row, 2)?, sql::counter(row, 3)?))
        })?
        .collect::<fastctx_storage::rusqlite::Result<Vec<_>>>()?;
    match rows.as_slice() {
        [] => Ok(None),
        [(stream, epoch, sequence, local)] => Ok(Some(Stream {
            stream: StreamId::try_from(stream.clone()).map_err(sql::invalid)?,
            epoch: ProducerEpoch::try_from(epoch.clone()).map_err(sql::invalid)?,
            sequence: Sequence(*sequence),
            local: LocalSequence(*local),
        })),
        _ => Err(sql::invalid("a conversation timeline with a producer handover installs through the hub's path")),
    }
}

/// Up to `max` committed records after `after`, in stream order.
pub(super) fn records(
    db: &Connection,
    conversation: &ObjectKey,
    stream: &Stream,
    after: Sequence,
    max: u32,
) -> Result<Vec<Record>> {
    let scope = &conversation.scope;
    db.prepare_cached("SELECT sequence,payload,batch_proof FROM sync_stream_records WHERE hub=?1 AND scope=?2 AND stream=?3 AND epoch=?4 AND sequence>?5 ORDER BY sequence LIMIT ?6")?
        .query_map(
            params![
                scope.hub.to_string(),
                scope.scope.as_str(),
                stream.stream.as_str(),
                stream.epoch.as_str(),
                after.0.to_be_bytes().as_slice(),
                max
            ],
            |row| Ok((sql::counter(row, 0)?, row.get::<_, Vec<u8>>(1)?, row.get::<_, String>(2)?)),
        )?
        .map(|row| {
            let (sequence, payload, proof) = row?;
            Ok(Record {
                reference: RecordRef {
                    stream: stream.stream.clone(),
                    epoch: stream.epoch.clone(),
                    sequence: Sequence(sequence),
                },
                payload: decode(&payload)?,
                proof: Digest::try_from(proof).map_err(sql::invalid)?,
            })
        })
        .collect()
}

/// The stored statement of a source proof, exactly as signed.
pub(super) fn proof(db: &Connection, conversation: &ObjectKey, proof: &Digest) -> Result<SignedSourceBatch> {
    let scope = &conversation.scope;
    let (purpose, signer, data, signature) = db
        .prepare_cached("SELECT purpose,signer,signed_bytes,signature FROM sync_source_proofs WHERE hub=?1 AND scope=?2 AND proof=?3")?
        .query_row(params![scope.hub.to_string(), scope.scope.as_str(), proof.as_str()], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?, row.get::<_, Vec<u8>>(2)?, row.get::<_, Vec<u8>>(3)?))
        })?;
    let statement = SignedStatement {
        purpose: ProofPurpose::from_domain(&purpose).ok_or_else(|| sql::invalid("unknown stored proof purpose"))?,
        signer: SigningKey(signer.try_into().map_err(|_| sql::invalid("invalid stored proof signer"))?),
        data: Bytes(data),
        signature: Signature(signature.try_into().map_err(|_| sql::invalid("invalid stored proof signature"))?),
    };
    if &statement.id() != proof {
        return Err(sql::invalid("a stored source proof differs from its identity"));
    }
    Ok(SignedSourceBatch { proof: statement })
}

/// Installation and reading keep replica custody of the conversation under the scope's
/// current holding period.
pub(super) fn custody(
    db: &Connection,
    holder: &crate::cursor::Holder,
    conversation: &ObjectKey,
) -> fastctx_storage::Result<Custody> {
    let scope = &conversation.scope;
    let (acl, epoch) = db
        .prepare_cached("SELECT acl_revision,publication_epoch FROM sync_scopes WHERE hub=?1 AND scope=?2")?
        .query_row(params![scope.hub.to_string(), scope.scope.as_str()], |row| {
            Ok((sql::counter(row, 0)?, sql::counter(row, 1)?))
        })?;
    Ok(Custody {
        context: AccessContext {
            principal: Principal::LocalService,
            scope: scope.clone(),
            observed_acl: Revision(acl),
        },
        kind: REGISTRY.asset(KIND).map_err(sql::invalid)?.registered(),
        object: conversation.clone(),
        purpose: HoldingUse::Replica,
        holding: HoldingPeriod {
            holder: holder.clone(),
            scope: scope.clone(),
            acl_revision: Revision(acl),
            publication_epoch: Revision(epoch),
        },
    })
}

/// The fence's view of the stream: local until the hub confirms its epoch and
/// the prefix digest reaches the installed cursor.
pub(super) fn progress(db: &Connection, conversation: &ObjectKey, through: &StreamCursor) -> Result<StreamProgress> {
    let scope = &conversation.scope;
    let confirmed = db
        .prepare_cached("SELECT h.epoch_proof,r.prefix_digest FROM sync_stream_heads h JOIN sync_stream_records r ON r.hub=h.hub AND r.scope=h.scope AND r.stream=h.stream AND r.epoch=h.epoch WHERE h.hub=?1 AND h.scope=?2 AND h.stream=?3 AND h.epoch=?4 AND r.sequence=?5")?
        .query_row(
            params![
                scope.hub.to_string(),
                scope.scope.as_str(),
                through.stream.as_str(),
                through.epoch.as_str(),
                through.committed.0.to_be_bytes().as_slice()
            ],
            |row| Ok((row.get::<_, Option<String>>(0)?, row.get::<_, Option<String>>(1)?)),
        )
        .optional()?;
    Ok(match confirmed {
        Some((Some(proof), Some(prefix))) => StreamProgress::Confirmed {
            cursor: through.clone(),
            epoch_proof: Digest::try_from(proof).map_err(sql::invalid)?,
            prefix_digest: Digest::try_from(prefix).map_err(sql::invalid)?,
        },
        _ => StreamProgress::LocalPending { cursor: through.clone() },
    })
}

/// Whether `fence` is still a live hold on exactly the snapshot it names: the
/// use exists unreleased and unexpired, and binds this conversation, the
/// fence's root and its projection. A fence is client data, so its lease id
/// alone proves nothing about the root a read would follow.
pub(super) fn fence_held(db: &Connection, fence: &TimelineFence, now: i64) -> Result<bool> {
    let scope = &fence.conversation.scope;
    let row = db
        .prepare_cached("SELECT object,kind,hash,root_kind,usage,holding FROM sync_root_leases WHERE hub=?1 AND scope=?2 AND lease=?3 AND released=0 AND supply_id IS NULL AND (expires IS NULL OR expires>?4)")?
        .query_row(params![scope.hub.to_string(), scope.scope.as_str(), fence.lease.as_str(), now], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Vec<u8>>(4)?,
                row.get::<_,Vec<u8>>(5)?,
            ))
        })
        .optional()?;
    let Some((object, kind, hash, form, usage, holding)) = row else {
        return Ok(false);
    };
    let holding: HoldingPeriod = decode(&holding)?;
    let Some(current) = crate::store::scopes::current(db, scope)?.filter(|value| value.enabled) else {
        return Ok(false);
    };
    if holding.scope != *scope {
        return Err(sql::invalid("timeline fence belongs to another scope"));
    }
    if holding.acl_revision != current.acl_revision || holding.publication_epoch != current.publication_epoch {
        return Err(fastctx_storage::Error::with_kind(
            fastctx_storage::ErrorKind::Busy,
            "timeline fence is awaiting holding-period reconciliation",
        ));
    }
    let usage: RootUse = decode(&usage)?;
    Ok(object == fence.conversation.object.as_str()
        && kind == KIND
        && hash == fence.root.0.digest.as_str()
        && form == "manifest"
        && usage == RootUse::Timeline { projection: fence.projection })
}
