//! Authority heads retain their original proof; local availability is independent.
use super::rows::{self, encode};
use crate::{
    blocks::{SqliteBlocks, replica_commit},
    catalog::{CatalogChange, HeadEvidence, Lifecycle},
    cursor::Completeness,
    identity::{Digest, StreamId},
    manifest::ContentRef,
    proof::{CommittedFragment, ProofVerifier, SignedAcceptanceFragment},
    registry::ConflictRule,
    store::{Error, access::Decision, local_head, mutation::Origin, replica::ReplicaContext, sql},
};
use fastctx_storage::{
    Result,
    rusqlite::{Connection, OptionalExtension, params},
};
use std::collections::BTreeMap;

pub(super) struct AcceptedEvidence {
    checked: CommittedFragment,
    signed: SignedAcceptanceFragment,
}
pub(super) fn verify(
    verifier: &dyn ProofVerifier,
    change: &CatalogChange,
    proofs: &BTreeMap<Digest, SignedAcceptanceFragment>,
) -> std::result::Result<Option<AcceptedEvidence>, Error> {
    let head = &change.head;
    let HeadEvidence::Accepted { proof } = &head.evidence else { return Ok(None) };
    let signed =
        proofs.get(proof).ok_or_else(|| Error::Invalid("accepted head has no original proof on its page".into()))?;
    let checked = verifier.accepted(signed, &head.authority)?;
    checked.validate().map_err(Error::Invalid)?;
    let evidence = AcceptedEvidence { checked, signed: signed.clone() };
    match_head(change, &evidence)?;
    Ok(Some(evidence))
}
pub(super) fn match_head(change: &CatalogChange, evidence: &AcceptedEvidence) -> std::result::Result<(), Error> {
    let head = &change.head;
    let checked = &evidence.checked;
    let f = &checked.fragment;
    let object = f
        .objects
        .iter()
        .find(|o| o.key == head.key)
        .ok_or_else(|| Error::Invalid("acceptance does not name this object".into()))?;
    if f.binding.hub != head.key.scope.hub
        || f.scope != head.key.scope.scope
        || f.kind != head.kind
        || f.schema != head.schema
        || f.binding.authority != head.authority
        || f.binding.operation != change.operation
        || object.allocated != head.revision
        || object.current_version != head.current_version
        || object.current_content != head.content
        || object.history != head.history
        || object.lifecycle != head.lifecycle
    {
        return Err(Error::Invalid("head differs from its original acceptance".into()));
    }
    Ok(())
}

pub(super) fn install(
    db: &Connection,
    blocks: &SqliteBlocks,
    context: &ReplicaContext,
    change: &CatalogChange,
    accepted: Option<&AcceptedEvidence>,
) -> Result<bool> {
    let head = &change.head;
    let custody = rows::custody(context, &head.key, &head.kind)?;
    let registration = custody.kind.registration();
    if registration.schema != head.schema
        || (registration.conflict == ConflictRule::SingleWriter) != matches!(head.evidence, HeadEvidence::Stream { .. })
    {
        return Err(sql::invalid("object evidence does not match its registered schema and conflict rule"));
    }
    match blocks.policy().can_keep(&custody.context, custody.kind, &custody.object, &custody.purpose) {
        Decision::Allowed { revision } if revision == context.holding.acl_revision => {}
        Decision::Denied { revision, .. } if revision == context.holding.acl_revision => return Ok(false),
        _ => {
            return Err(fastctx_storage::Error::with_kind(
                fastctx_storage::ErrorKind::Busy,
                "replica custody snapshot has not reached the scope fence",
            ));
        }
    }
    let hub = head.key.scope.hub.to_string();
    let scope = head.key.scope.scope.as_str();
    let object = head.key.object.as_str();
    let existing: Option<String> = db
        .query_row(
            "SELECT kind FROM sync_objects WHERE hub=?1 AND scope=?2 AND object=?3",
            params![hub, scope, object],
            |r| r.get(0),
        )
        .optional()?;
    if existing.as_ref().is_some_and(|kind| kind != &head.kind) {
        return Err(sql::invalid("an existing object cannot change its kind"));
    }
    let deleted: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sync_tombstones WHERE hub=?1 AND scope=?2 AND object=?3)",
        params![hub, scope, object],
        |r| r.get(0),
    )?;
    if deleted && head.lifecycle != Lifecycle::Deleted {
        return Err(sql::invalid("a deleted object cannot be resurrected by a feed head"));
    }
    if !deleted {
        replica_commit::authorize(db, blocks, &custody)?;
    }
    if let Some(evidence) = accepted {
        let checked = &evidence.checked;
        let value = checked
            .fragment
            .objects
            .iter()
            .find(|v| v.key == head.key)
            .ok_or_else(|| sql::invalid("verified head has no accepted object"))?;
        let epoch = value.allocated.authority_epoch.bytes();
        let revision = value.allocated.revision.0.to_be_bytes();
        let encoded = encode(value)?;
        let existing:Option<(Vec<u8>,String)>=db.query_row("SELECT accepted,proof FROM sync_replica_versions WHERE hub=?1 AND scope=?2 AND object=?3 AND authority_epoch=?4 AND revision=?5",params![hub,scope,object,epoch.as_slice(),revision.as_slice()],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
        if existing.as_ref().is_some_and(|(old, proof)| old != &encoded || proof != evidence.signed.proof.id().as_str())
        {
            return Err(sql::invalid("an accepted version identity has different contents"));
        }
        let proof = evidence.signed.proof.id();
        db.execute(
            "INSERT INTO sync_replica_proofs(hub,scope,proof,original) VALUES(?1,?2,?3,?4) ON CONFLICT DO NOTHING",
            params![hub, scope, proof.as_str(), encode(&evidence.signed)?],
        )?;
        db.execute("INSERT INTO sync_replica_versions(hub,scope,object,authority_epoch,revision,accepted,proof) VALUES(?1,?2,?3,?4,?5,?6,?7) ON CONFLICT DO NOTHING",params![hub,scope,object,epoch.as_slice(),revision.as_slice(),encoded,proof.as_str()])?;
        let actor = serde_json::to_string(&checked.fragment.binding.actor).map_err(|e| sql::invalid(e.to_string()))?;
        let stored_version=db.query_row("SELECT actor,operation,content,parents,state FROM sync_object_versions WHERE hub=?1 AND scope=?2 AND object=?3 AND authority_epoch=?4 AND revision=?5",params![hub,scope,object,epoch.as_slice(),revision.as_slice()],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,Option<Vec<u8>>>(2)?,r.get::<_,Vec<u8>>(3)?,r.get::<_,String>(4)?))).optional()?;
        let expected_version = (
            actor.clone(),
            change.operation.as_str().to_owned(),
            value.content.as_ref().map(encode).transpose()?,
            encode(&value.parents)?,
            lifecycle(value.lifecycle).to_string(),
        );
        if stored_version.as_ref().is_some_and(|existing| existing != &expected_version) {
            return Err(sql::invalid("accepted version differs from the existing version index"));
        }
        db.execute("INSERT INTO sync_object_versions(hub,scope,object,authority_epoch,revision,actor,operation,content,parents,state) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10) ON CONFLICT DO NOTHING",params![hub,scope,object,epoch.as_slice(),revision.as_slice(),actor,change.operation.as_str(),value.content.as_ref().map(encode).transpose()?,encode(&value.parents)?,lifecycle(value.lifecycle)])?;
        for root in [&value.content, &value.current_content]
            .into_iter()
            .flatten()
            .cloned()
            .chain(value.history.clone().map(|root| ContentRef::Manifest { root }))
        {
            let (form, length) = replica_commit::shape(&root);
            db.execute("INSERT INTO sync_asset_roots(hub,scope,object,authority_epoch,revision,hash,root_kind,decoded_bytes) VALUES(?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT DO NOTHING",params![hub,scope,object,epoch.as_slice(),revision.as_slice(),root.id().digest.as_str(),form,length])?;
            replica_commit::adopt(db, &custody, &root)?;
        }
    }
    let state = lifecycle(head.lifecycle);
    db.execute("INSERT INTO sync_objects(hub,scope,object,kind,schema,authority,evidence,authority_epoch,revision,current_authority_epoch,current_version,content,history,state,completeness) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15) ON CONFLICT(hub,scope,object) DO UPDATE SET schema=excluded.schema,authority=excluded.authority,evidence=excluded.evidence,authority_epoch=excluded.authority_epoch,revision=excluded.revision,current_authority_epoch=excluded.current_authority_epoch,current_version=excluded.current_version,content=excluded.content,history=excluded.history,state=excluded.state,completeness=excluded.completeness",params![hub,scope,object,head.kind,head.schema,encode(&head.authority)?,encode(&head.evidence)?,head.revision.authority_epoch.bytes().as_slice(),head.revision.revision.0.to_be_bytes().as_slice(),head.current_version.authority_epoch.bytes().as_slice(),head.current_version.revision.0.to_be_bytes().as_slice(),head.content.as_ref().map(encode).transpose()?,head.history.as_ref().map(|r|r.0.digest.as_str()),state,encode(&Completeness::MetadataOnly)?])?;
    let local = local_head::advance(db)?;
    let origin = Origin::Replicated { source: context.source.clone() };
    db.execute("INSERT INTO sync_local_changes(sequence,hub,scope,object,kind,authority_epoch,revision,origin,operation) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",params![local.0.to_be_bytes().as_slice(),hub,scope,object,head.kind,head.revision.authority_epoch.bytes().as_slice(),head.revision.revision.0.to_be_bytes().as_slice(),encode(&origin)?,change.operation.as_str()])?;
    if head.lifecycle == Lifecycle::Deleted {
        db.execute("INSERT INTO sync_tombstones(hub,scope,object,authority_epoch,revision,operation) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT DO NOTHING",params![hub,scope,object,head.revision.authority_epoch.bytes().as_slice(),head.revision.revision.0.to_be_bytes().as_slice(),change.operation.as_str()])?;
        db.execute(
            "UPDATE sync_replica_streams SET pending=0 WHERE hub=?1 AND scope=?2 AND object=?3",
            params![hub, scope, object],
        )?;
        return Ok(true);
    }
    if let HeadEvidence::Stream { epoch, through, prefix_digest } = &head.evidence {
        if head.content.is_some() || head.history.is_some() {
            return Err(sql::invalid("a stream head carries positions, not unsigned body roots"));
        }
        let stream = StreamId::of(&head.key, &head.kind);
        let old:Option<(u64,Option<String>)>=db.query_row("SELECT wanted,prefix_digest FROM sync_replica_streams WHERE hub=?1 AND scope=?2 AND stream=?3 AND epoch_proof=?4",params![hub,scope,stream.as_str(),epoch.as_str()],|r|Ok((sql::counter(r,0)?,r.get(1)?))).optional()?;
        if old.as_ref().is_some_and(|(seq, prefix)| {
            *seq > through.0 || *seq == through.0 && prefix.as_ref().is_some_and(|p| p != prefix_digest.as_str())
        }) {
            return Err(sql::invalid("a stream target cannot rewind or fork within one epoch"));
        }
        db.execute("INSERT INTO sync_replica_streams(hub,scope,stream,epoch_proof,object,kind,schema,holding,purpose,wanted,prefix_digest) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11) ON CONFLICT(hub,scope,stream,epoch_proof) DO UPDATE SET wanted=excluded.wanted,prefix_digest=excluded.prefix_digest,holding=excluded.holding,purpose=excluded.purpose,pending=1",params![hub,scope,stream.as_str(),epoch.as_str(),object,head.kind,head.schema,encode(&custody.holding)?,encode(&custody.purpose)?,through.0.to_be_bytes().as_slice(),prefix_digest.as_str()])?;
        crate::scheduler::replica_changed(
            db,
            &custody.holding,
            format!("replica-stream/{}/{}", stream.as_str(), epoch.as_str()),
            local,
        )?;
    }
    Ok(true)
}
fn lifecycle(value: Lifecycle) -> &'static str {
    match value {
        Lifecycle::Live => "live",
        Lifecycle::Removed => "removed",
        Lifecycle::Deleted => "deleted",
    }
}
