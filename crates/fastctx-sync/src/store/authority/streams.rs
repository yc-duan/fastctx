//! The accepted stream is a consecutive sequence, hashed per record, not per batch.
use super::{SourceReceipt, encode};
use crate::{
    catalog::{Authority, HeadEvidence, Lifecycle, ObjectHead, SourceBatch},
    cursor::{Completeness, Revision, Sequence, VersionRef},
    identity::Digest,
    proof::ConfirmedEpoch,
    store::{control, epochs, local_head, sql},
};
use fastctx_storage::{
    Result,
    rusqlite::{Connection, OptionalExtension, params},
};
use fastctx_types::sync::feed;

pub(super) struct Head {
    pub sequence: Sequence,
    pub prefix: Digest,
    pub proof: ConfirmedEpoch,
}
pub(super) fn head(db: &Connection, batch: &SourceBatch) -> Result<Option<Head>> {
    let another: bool = db
        .prepare_cached(
            "SELECT EXISTS(SELECT 1 FROM sync_epoch_descriptors WHERE hub=?1 AND scope=?2 AND stream=?3 AND epoch!=?4)",
        )?
        .query_row(
            params![
                batch.scope.hub.to_string(),
                batch.scope.scope.as_str(),
                batch.stream.as_str(),
                batch.epoch.as_str()
            ],
            |r| r.get(0),
        )?;
    if another {
        return Err(sql::invalid("a new producer epoch requires its confirmed handover"));
    }
    let mut query=db.prepare_cached("SELECT epoch,object,producer,kind,schema,sequence,prefix_digest FROM sync_stream_heads WHERE hub=?1 AND scope=?2 AND stream=?3 LIMIT 2")?;
    let rows = query
        .query_map(params![batch.scope.hub.to_string(), batch.scope.scope.as_str(), batch.stream.as_str()], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Vec<u8>>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, u16>(4)?,
                sql::counter(r, 5)?,
                r.get::<_, Option<String>>(6)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let [(epoch, _, producer, kind, schema, sequence, prefix)] = rows.as_slice() else {
        return if rows.is_empty() {
            Ok(None)
        } else {
            Err(sql::invalid("a source stream needs its confirmed handover"))
        };
    };
    if epoch != batch.epoch.as_str()
        || producer != &encode(&batch.producer)?
        || kind != &batch.kind
        || *schema != batch.schema
    {
        return Err(sql::invalid("another producer epoch needs a confirmed handover"));
    }
    let proof = epochs::get(db, &batch.scope, &batch.stream, &batch.epoch)?
        .ok_or_else(|| sql::invalid("an authority source head has no epoch evidence"))?;
    Ok(Some(Head {
        sequence: Sequence(*sequence),
        prefix: prefix
            .clone()
            .ok_or_else(|| sql::invalid("an accepted stream has no prefix"))?
            .try_into()
            .map_err(sql::invalid)?,
        proof,
    }))
}
pub(super) fn append(
    db: &Connection,
    authority: &super::SqliteAuthority,
    prepared: &super::prepare::Prepared,
) -> Result<SourceReceipt> {
    let batch = &prepared.batch;
    let previous = head(db, batch)?;
    let (epoch, prefix) = match previous {
        Some(head) => (head.proof, head.prefix),
        None => {
            let descriptor = super::prepare::initial(batch, control::next_control(db, authority.run.hub())?);
            let proof = authority.signer.epoch(&descriptor).map_err(|e| sql::invalid(e.to_string()))?;
            let verified =
                authority.verifier.epoch(&proof, &authority.identity).map_err(|e| sql::invalid(e.to_string()))?;
            if verified != descriptor {
                return Err(sql::invalid("hub signer returned another epoch descriptor"));
            }
            epochs::put(db, &descriptor, &proof)?;
            let prefix = feed::stream_genesis(&proof.proof.id());
            (proof, prefix)
        }
    };
    let local = local_head::advance(db)?;
    let (hub, scope, stream, name) =
        (batch.scope.hub.to_string(), batch.scope.scope.as_str(), batch.stream.as_str(), batch.epoch.as_str());
    let proof = prepared.signed.proof.id();
    db.prepare_cached("INSERT INTO sync_stream_heads(hub,scope,stream,object,producer,epoch,kind,schema,sequence,local,epoch_proof,prefix_digest) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12) ON CONFLICT(hub,scope,stream,epoch) DO UPDATE SET sequence=excluded.sequence,local=excluded.local")?
        .execute(params![hub,scope,stream,prepared.bound.hub.object.object.as_str(),encode(&batch.producer)?,name,batch.kind,batch.schema,
            batch.through.0.to_be_bytes().as_slice(),local.0.to_be_bytes().as_slice(),epoch.proof.id().as_str(),prefix.as_str()])?;
    write_proof(db, batch, &prepared.signed, "accepted")?;
    let mut prefix = prefix;
    for record in &batch.records {
        prefix = feed::stream_advance(&prefix, record).map_err(sql::invalid)?.1;
        db.prepare_cached("INSERT INTO sync_stream_records(hub,scope,stream,epoch,sequence,operation,kind,schema,payload,causes,batch_proof,prefix_digest) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)")?
            .execute(params![hub,scope,stream,name,record.sequence.0.to_be_bytes().as_slice(),record.operation.as_str(),record.kind,record.schema,
                encode(&record.payload)?,encode(&record.causes)?,proof.as_str(),prefix.as_str()])?;
        for root in std::iter::once(crate::manifest::ContentRef::from(record.payload.clone()))
            .chain(record.roots.iter().cloned())
        {
            let (form, length) = match &root {
                crate::manifest::ContentRef::Single { decoded_bytes, .. } => ("block", Some(*decoded_bytes)),
                _ => ("manifest", None),
            };
            db.prepare_cached("INSERT INTO sync_stream_roots(hub,scope,stream,epoch,sequence,hash,root_kind,decoded_bytes) VALUES(?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT DO NOTHING")?
                .execute(params![hub,scope,stream,name,record.sequence.0.to_be_bytes().as_slice(),root.id().digest.as_str(),form,length])?;
        }
    }
    db.prepare_cached(
        "UPDATE sync_stream_heads SET prefix_digest=?5 WHERE hub=?1 AND scope=?2 AND stream=?3 AND epoch=?4",
    )?
    .execute(params![hub, scope, stream, name, prefix.as_str()])?;
    let version = version(db, authority, prepared)?;
    let crate::store::access::Principal::Account { account, device } = &prepared.bound.source.context.principal else {
        return Err(sql::invalid("accepted source has no authenticated actor"));
    };
    let head = ObjectHead {
        key: prepared.bound.hub.object.clone(),
        kind: batch.kind.clone(),
        schema: batch.schema,
        acl_revision: prepared.bound.source.context.observed_acl,
        authority: Authority::Device { device: device.clone(), epoch: batch.epoch.clone() },
        evidence: HeadEvidence::Stream {
            epoch: epoch.proof.id(),
            through: batch.through,
            prefix_digest: prefix.clone(),
        },
        revision: version.clone(),
        current_version: version,
        content: None,
        history: None,
        lifecycle: Lifecycle::Live,
        completeness: Completeness::MetadataOnly,
    };
    let scope_cursor = crate::store::feed::publish(db, &batch.operation, &head)?;
    let receipt = SourceReceipt {
        operation: batch.operation.clone(),
        actor: crate::proof::AcceptedActor::Account { account: account.clone(), device: device.clone() },
        proof,
        epoch,
        stream: crate::cursor::StreamCursor {
            scope: batch.scope.clone(),
            stream: batch.stream.clone(),
            epoch: batch.epoch.clone(),
            committed: batch.through,
        },
        prefix_digest: prefix,
        head,
        scope: scope_cursor,
    };
    super::waiting::dirty(db, batch, local)?;
    Ok(receipt)
}
fn version(
    db: &Connection,
    authority: &super::SqliteAuthority,
    prepared: &super::prepare::Prepared,
) -> Result<VersionRef> {
    let object = &prepared.bound.hub.object;
    let previous = db
        .prepare_cached(
            "SELECT kind,authority,revision,state FROM sync_objects WHERE hub=?1 AND scope=?2 AND object=?3",
        )?
        .query_row(params![object.scope.hub.to_string(), object.scope.scope.as_str(), object.object.as_str()], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Vec<u8>>(1)?, sql::counter(r, 2)?, r.get::<_, String>(3)?))
        })
        .optional()?;
    let revision = match previous {
        None => 1,
        Some((kind, by, revision, state)) => {
            let crate::identity::Producer::Device { device } = &prepared.batch.producer else {
                return Err(sql::invalid("not a device source"));
            };
            let expected = Authority::Device { device: device.clone(), epoch: prepared.batch.epoch.clone() };
            if kind != prepared.batch.kind || by != encode(&expected)? || state != "live" {
                return Err(sql::invalid("source object changed kind, authority or lifecycle"));
            }
            revision.checked_add(1).ok_or_else(|| sql::invalid("source object revision overflow"))?
        }
    };
    Ok(VersionRef { authority_epoch: authority.run.epoch().clone(), revision: Revision(revision) })
}
pub(super) fn write_proof(
    db: &Connection,
    batch: &SourceBatch,
    signed: &crate::proof::SignedSourceBatch,
    state: &str,
) -> Result<()> {
    db.prepare_cached("INSERT INTO sync_source_proofs(hub,scope,proof,stream,epoch,producer,operation,first_sequence,last_sequence,purpose,signer,signed_bytes,signature,state) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14) ON CONFLICT(hub,scope,proof) DO UPDATE SET state=excluded.state")?
        .execute(params![batch.scope.hub.to_string(),batch.scope.scope.as_str(),signed.proof.id().as_str(),batch.stream.as_str(),batch.epoch.as_str(),
            encode(&batch.producer)?,batch.operation.as_str(),(batch.after.0+1).to_be_bytes().as_slice(),batch.through.0.to_be_bytes().as_slice(),
            signed.proof.purpose.domain(),signed.proof.signer.0.as_slice(),signed.proof.data.0,signed.proof.signature.0.as_slice(),state])?;
    Ok(())
}
