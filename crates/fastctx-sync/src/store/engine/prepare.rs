//! Semantic work and body IO finish before the SQL revision fence is acquired.
use super::{SqliteBackend, access, bodies, rows};
use crate::{
    catalog::ObjectHead,
    identity::Digest,
    proof::AcceptedActor,
    registry::{Asset, Kind},
    store::{
        Error,
        access::{AccessContext, HoldingUse, Principal},
        mutation::{AtomicMutation, OriginalIntent, ProposedObject, Submission},
    },
};

pub(super) struct Prepared {
    pub request: Submission,
    pub actor: AcceptedActor,
    pub hash: Digest,
    pub objects: Vec<Object>,
    pub bytes: usize,
}
pub(super) struct Object {
    pub proposal: ProposedObject,
    pub kind: Kind<Asset>,
    pub current: Option<ObjectHead>,
    pub custody: crate::store::access::Custody,
    pub before: Vec<u8>,
    pub after: Vec<u8>,
    pub references: Vec<crate::manifest::ContentRef>,
}
pub(super) fn actor(principal: &Principal) -> Result<AcceptedActor, Error> {
    match principal {
        Principal::Account { account, device } => {
            Ok(AcceptedActor::Account { account: account.clone(), device: device.clone() })
        }
        Principal::Agent { account, device, conversation } => Ok(AcceptedActor::Agent {
            account: account.clone(),
            device: device.clone(),
            conversation: conversation.clone(),
        }),
        Principal::LocalService => Err(Error::Denied("maintenance cannot impersonate an asset editor".into())),
    }
}
pub(crate) fn fingerprint(request: &Submission) -> Result<Digest, Error> {
    use sha2::{Digest as _, Sha256};
    let mut hash = Sha256::new();
    hash.update(b"fastctx-sync/submission/v1\0");
    let mut value = serde_json::to_value(request).map_err(|e| Error::Invalid(e.to_string()))?;
    sort_fields(&mut value);
    hash.update(serde_json::to_vec(&value).map_err(|e| Error::Invalid(e.to_string()))?);
    Ok(Digest::from_bytes(hash.finalize().into()))
}
fn sort_fields(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(fields) => {
            let sorted: std::collections::BTreeMap<_, _> = std::mem::take(fields).into_iter().collect();
            for (key, mut value) in sorted {
                sort_fields(&mut value);
                fields.insert(key, value);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                sort_fields(item);
            }
        }
        _ => {}
    }
}
pub(super) async fn prepare(
    store: &SqliteBackend,
    principal: Principal,
    mutation: &AtomicMutation,
) -> Result<Prepared, Error> {
    let actor = actor(&principal)?;
    let origin = match &actor {
        AcceptedActor::Account { device, .. } | AcceptedActor::Agent { device, .. } => device,
    };
    let request = mutation.request().clone();
    if request.origin != *origin {
        return Err(Error::Denied("submission origin differs from the authenticated actor".into()));
    }
    let hash = fingerprint(&request)?;
    let mut prepared = Prepared { request: request.clone(), actor, hash, objects: Vec::new(), bytes: 0 };
    let mut roots = 0_usize;
    for (proposal, kind) in request.objects.iter().zip(mutation.kinds()) {
        if proposal.key.scope.hub != store.hub {
            return Err(Error::Denied("submission belongs to another hub".into()));
        }
        let handler = store
            .handlers
            .get(proposal.kind.as_str())
            .ok_or_else(|| Error::Unavailable("this asset kind has no installed handler".into()))?;
        if !store.authorizers.contains_key(proposal.kind.as_str()) {
            return Err(Error::Denied("this asset kind has no semantic write authorizer".into()));
        }
        let context = AccessContext {
            principal: principal.clone(),
            scope: proposal.key.scope.clone(),
            observed_acl: proposal.observed_acl,
        };
        let (current, custody) = store
            .db
            .read_now(|db| {
                access::current(db, &context.scope, context.observed_acl)?;
                access::allowed(store.policy.can_write(&context, kind.registered()), context.observed_acl)?;
                let custody = access::custody(
                    db,
                    &store.holder,
                    context.clone(),
                    kind.registered(),
                    proposal.key.clone(),
                    HoldingUse::Replica,
                )?;
                access::allowed(
                    store.policy.can_keep(&context, kind.registered(), &proposal.key, &custody.purpose),
                    context.observed_acl,
                )?;
                let current = rows::get(db, &proposal.key)?;
                super::basis::verify(db, &request.origin, proposal, current.as_ref())?;
                Ok((current, custody))
            })
            .map_err(|e| crate::store::sql::error(e, request.operation.clone()))?;
        if current.as_ref().is_some_and(|head| head.kind != proposal.kind) {
            return Err(Error::Invalid("an object cannot change its registered kind".into()));
        }
        let mut before = Vec::new();
        let mut after = Vec::new();
        for (reference, target) in [(proposal.before.as_ref(), &mut before), (proposal.candidate.as_ref(), &mut after)]
        {
            if let Some(reference) = reference {
                *target = bodies::read(
                    store.blocks.as_ref(),
                    custody.clone(),
                    reference,
                    store.semantic_bytes.saturating_sub(prepared.bytes),
                )
                .await?;
                prepared.bytes = prepared.bytes.checked_add(target.len()).ok_or(Error::Busy)?;
            }
        }
        if !matches!(proposal.original, OriginalIntent::Remove | OriginalIntent::Delete) && proposal.candidate.is_none()
        {
            return Err(Error::Invalid("a body edit needs a candidate content reference".into()));
        }
        let schema = kind.registration().schema;
        if proposal.candidate.is_some() {
            handler.validate(schema, &after)?;
        }
        match &proposal.original {
            OriginalIntent::Write { content, schema: written }
                if proposal.candidate.as_ref() == Some(content) && *written == schema => {}
            OriginalIntent::Edit { edits } => {
                let changed =
                    handler.apply_edit(schema, &before, edits, store.semantic_bytes.saturating_sub(prepared.bytes))?;
                if changed != after {
                    return Err(Error::Invalid("candidate body does not implement its declared edit".into()));
                }
            }
            OriginalIntent::Fields { edits } => {
                let patched = handler.patch_fields(schema, &before, edits)?;
                let expected: serde_json::Value =
                    serde_json::from_slice(&patched).map_err(|e| Error::Invalid(e.to_string()))?;
                let candidate: serde_json::Value =
                    serde_json::from_slice(&after).map_err(|e| Error::Invalid(e.to_string()))?;
                if expected != candidate {
                    return Err(Error::Invalid("candidate changes fields outside its declared patch".into()));
                }
            }
            OriginalIntent::Remove | OriginalIntent::Delete if proposal.candidate.is_none() => {}
            _ => return Err(Error::Invalid("intent and candidate do not match this installed handler".into())),
        }
        let references = if proposal.candidate.is_some() { handler.references(schema, &after)? } else { Vec::new() };
        let base = matches!(proposal.base, crate::store::mutation::BaseVersion::Version { root: Some(_), .. });
        roots = roots
            .checked_add(
                references.len()
                    + usize::from(base)
                    + usize::from(proposal.before.is_some())
                    + usize::from(proposal.candidate.is_some()),
            )
            .ok_or(Error::Busy)?;
        if roots > 4097 {
            return Err(Error::Invalid("handler references exceed the operation budget".into()));
        }
        for reference in &references {
            reference.validate().map_err(Error::Invalid)?;
            if reference.id().domain != proposal.key.scope {
                return Err(Error::Invalid("handler reference crosses its scope".into()));
            }
        }
        prepared.objects.push(Object {
            proposal: proposal.clone(),
            kind: *kind,
            current,
            custody,
            before,
            after,
            references,
        });
    }
    Ok(prepared)
}
