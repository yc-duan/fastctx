//! Pending edits, retained roots, outgoing intent and local notices commit together.
use super::{
    SqliteBackend, access,
    prepare::{self, Prepared},
    rows,
};
use crate::{
    cursor::LocalSequence,
    manifest::ContentRef,
    store::{
        Error,
        access::{AccessContext, HoldingUse, Principal},
        authorize::PreparedChange,
        boundary::{self, Role},
        mutation::{AtomicMutation, BaseVersion, PendingState, SubmissionMode, SubmissionReceipt},
        sql,
    },
};
use fastctx_storage::{
    Result,
    rusqlite::{Connection, OptionalExtension, params},
};

pub(super) async fn submit(
    store: &SqliteBackend,
    principal: Principal,
    mutation: AtomicMutation,
) -> std::result::Result<SubmissionReceipt, Error> {
    let original = mutation.request();
    let actor = prepare::actor(&principal)?;
    let hash = prepare::fingerprint(original)?;
    let actor_key = String::from_utf8(rows::encode(&actor).map_err(|e| sql::error(e, original.operation.clone()))?)
        .map_err(|e| Error::Invalid(e.to_string()))?;
    let previous = store
        .db
        .read_now(|db| {
            for (object, kind) in original.objects.iter().zip(mutation.kinds()) {
                if object.key.scope.hub != store.hub {
                    return Err(sql::invalid("pending request belongs to another hub"));
                }
                let context = AccessContext {
                    principal: principal.clone(),
                    scope: object.key.scope.clone(),
                    observed_acl: object.observed_acl,
                };
                access::current(db, &context.scope, context.observed_acl)?;
                access::allowed(store.policy.can_write(&context, kind.registered()), context.observed_acl)?;
                access::allowed(
                    store.policy.can_keep(&context, kind.registered(), &object.key, &HoldingUse::Replica),
                    context.observed_acl,
                )?;
            }
            existing(db, original, &actor_key, &hash)
        })
        .map_err(|e| sql::error(e, original.operation.clone()))?;
    if let Some(receipt) = previous {
        return Ok(receipt);
    }
    let _admission = store.semantic_budget.acquire(original).await?;
    for attempt in 0..3 {
        let prepared = prepare::prepare(store, principal.clone(), &mutation).await?;
        let operation = prepared.request.operation.clone();
        let engine = store.clone();
        let notify = store.changed.clone();
        let result = store
            .db
            .write_then(
                prepared.bytes + prepared.field_bytes + fastctx_types::sync::PAGE_BYTES as usize,
                move |db| {
                    let _boundary = boundary::enter_typed_write(Role::Pending);
                    stage(db, &engine, prepared)
                },
                move |receipt| {
                    notify.send_replace(receipt.local);
                },
            )
            .await
            .map_err(|e| sql::error(e, operation));
        match result {
            Err(Error::Busy) if attempt < 2 => continue,
            result => return result,
        }
    }
    Err(Error::Busy)
}
fn existing(
    db: &Connection,
    request: &crate::store::mutation::Submission,
    actor: &str,
    hash: &crate::identity::Digest,
) -> Result<Option<SubmissionReceipt>> {
    let hub = &request.objects[0].key.scope.hub;
    let previous=db.prepare_cached("SELECT actor,request_hash,local_sequence FROM sync_pending_mutations WHERE hub=?1 AND origin=?2 AND operation=?3")?.query_row(params![hub.to_string(),request.origin.as_str(),request.operation.as_str()],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,sql::counter(r,2)?))).optional()?;
    previous
        .map(|(saved_actor, saved_hash, local)| {
            if saved_actor != actor || saved_hash != hash.as_str() {
                return Err(sql::invalid("operation ID was already used for another immutable request"));
            }
            Ok(SubmissionReceipt { operation: request.operation.clone(), local: LocalSequence(local), authority: None })
        })
        .transpose()
}
fn stage(db: &Connection, store: &SqliteBackend, prepared: Prepared) -> Result<SubmissionReceipt> {
    let actor = String::from_utf8(rows::encode(&prepared.actor)?).map_err(|e| sql::invalid(e.to_string()))?;
    let request = &prepared.request;
    for object in &prepared.objects {
        let context = &object.custody.context;
        let publication = access::current(db, &context.scope, context.observed_acl)?;
        if publication != object.custody.holding.publication_epoch {
            return Err(sql::invalid("pending edit lost its holding period"));
        }
        access::allowed(store.policy.can_write(context, object.kind.registered()), context.observed_acl)?;
        access::allowed(
            store.policy.can_keep(context, object.kind.registered(), &object.proposal.key, &object.custody.purpose),
            context.observed_acl,
        )?;
    }
    if let Some(receipt) = existing(db, request, &actor, &prepared.hash)? {
        return Ok(receipt);
    }
    for object in &prepared.objects {
        let context = &object.custody.context;
        let current = rows::get(db, &object.proposal.key)?;
        if current.as_ref().map(|h| &h.revision) != object.current.as_ref().map(|h| &h.revision) {
            return Err(fastctx_storage::Error::with_kind(
                fastctx_storage::ErrorKind::Busy,
                "object changed during semantic preparation",
            ));
        }
        super::basis::verify(db, &request.origin, &object.proposal, current.as_ref())?;
        let handler =
            store.handlers.get(object.proposal.kind.as_str()).ok_or_else(|| sql::invalid("kind has no handler"))?;
        let (fields, _) = super::semantics::derive(
            handler.as_ref(),
            object.kind.registration().schema,
            &object.before,
            &object.after,
            object.field_budget,
        )
        .map_err(|error| sql::invalid(error.to_string()))?;
        if fields != object.fields {
            return Err(sql::invalid("semantic field changes differ inside the write fence"));
        }
        let authorizer = store
            .authorizers
            .get(object.proposal.kind.as_str())
            .ok_or_else(|| sql::invalid("kind has no semantic authorizer"))?;
        access::allowed(
            authorizer.authorize(
                context,
                PreparedChange {
                    proposal: &object.proposal,
                    current: current.as_ref(),
                    before: &object.before,
                    after: &object.after,
                    fields: &fields,
                },
            ),
            context.observed_acl,
        )?;
    }
    let mut local = rows::local(db)?;
    let hub = request.objects[0].key.scope.hub.to_string();
    let body = rows::encode(request)?;
    db.prepare_cached("INSERT INTO sync_pending_mutations(hub,origin,operation,actor,local_sequence,request_hash,original_request,mode,state,preserve_offline) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)")?.execute(params![hub,request.origin.as_str(),request.operation.as_str(),actor,local.0.to_be_bytes().as_slice(),prepared.hash.as_str(),body,match request.mode {SubmissionMode::Online=>"online",SubmissionMode::Offline=>"offline"},rows::encode(&PendingState::Queued)?,request.mode==SubmissionMode::Offline])?;
    for object in &prepared.objects {
        let proposal = &object.proposal;
        db.prepare_cached("INSERT INTO sync_pending_objects(hub,origin,operation,scope,object,kind,base,before_root,candidate_root,predecessor,original_intent,holding) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)")?.execute(params![hub,request.origin.as_str(),request.operation.as_str(),proposal.key.scope.scope.as_str(),proposal.key.object.as_str(),proposal.kind,rows::encode(&proposal.base)?,proposal.before.as_ref().map(rows::encode).transpose()?,proposal.candidate.as_ref().map(rows::encode).transpose()?,proposal.predecessor.as_ref().map(|id|id.as_str()),rows::encode(&proposal.original)?,rows::encode(&object.custody.holding)?])?;
        let base = match &proposal.base {
            BaseVersion::Absent => None,
            BaseVersion::Version { root, .. } => root.as_ref(),
        };
        for (role, root) in
            [("base", base), ("before", proposal.before.as_ref()), ("candidate", proposal.candidate.as_ref())]
        {
            if let Some(root) = root {
                retain(db, object, request, root, role)?;
            }
        }
        for (index, reference) in object.references.iter().enumerate() {
            retain(db, object, request, reference, &format!("reference:{index}"))?;
        }
        local = rows::advance(db)?;
        db.prepare_cached("INSERT INTO sync_local_changes(sequence,hub,scope,object,kind,origin,operation) VALUES(?1,?2,?3,?4,?5,?6,?7)")?.execute(params![local.0.to_be_bytes().as_slice(),hub,proposal.key.scope.scope.as_str(),proposal.key.object.as_str(),proposal.kind,rows::encode(&crate::store::mutation::Origin::Local)?,request.operation.as_str()])?;
    }
    db.prepare_cached(
        "UPDATE sync_pending_mutations SET local_sequence=?1 WHERE hub=?2 AND origin=?3 AND operation=?4",
    )?
    .execute(params![
        local.0.to_be_bytes().as_slice(),
        hub,
        request.origin.as_str(),
        request.operation.as_str()
    ])?;
    db.prepare_cached("INSERT INTO sync_outbox(destination,hub,scope,operation,body,holding,priority,next_due) VALUES('authority',?1,?2,?3,?4,NULL,3,0)")?.execute(params![hub,request.objects[0].key.scope.scope.as_str(),request.operation.as_str(),body])?;
    Ok(SubmissionReceipt { operation: request.operation.clone(), local, authority: None })
}
fn retain(
    db: &Connection,
    object: &prepare::Object,
    request: &crate::store::mutation::Submission,
    root: &ContentRef,
    role: &str,
) -> Result<()> {
    crate::blocks::retain_supplied(db, &object.custody, root)?;
    let (form, length) = match root {
        ContentRef::Single { decoded_bytes, .. } => ("block", Some(*decoded_bytes)),
        ContentRef::Manifest { .. } => ("manifest", None),
    };
    db.prepare_cached("INSERT INTO sync_pending_roots(hub,origin,operation,scope,object,kind,hash,root_kind,decoded_bytes,role) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)")?.execute(params![object.proposal.key.scope.hub.to_string(),request.origin.as_str(),request.operation.as_str(),object.proposal.key.scope.scope.as_str(),object.proposal.key.object.as_str(),object.proposal.kind,root.id().digest.as_str(),form,length,role])?;
    Ok(())
}
