//! Pending writes must leave their immutable request, roots and delivery intent.
use crate::{
    manifest::ContentRef,
    store::{mutation::Submission, sql},
};
use fastctx_storage::{
    Result,
    guard::{Action, Change},
    rusqlite::{Connection, params, types::ValueRef},
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Default)]
pub(super) struct PendingAudit {
    requests: BTreeSet<(String, String, String)>,
    objects: BTreeSet<(String, String, String, String, String)>,
    notices: BTreeMap<(String, String, String, String), Vec<u8>>,
    outbound: BTreeMap<(String, String), Vec<u8>>,
}
fn text(change: &Change<'_>, column: usize) -> Result<String> {
    match change.after(column)? {
        ValueRef::Text(bytes) => std::str::from_utf8(bytes).map(str::to_owned).map_err(|e| sql::invalid(e.to_string())),
        _ => Err(sql::invalid("pending mutation identity is not text")),
    }
}
impl PendingAudit {
    pub(super) fn changed(&mut self, change: &Change<'_>) -> Result<()> {
        match change.table() {
            "sync_pending_mutations" if change.action() == Action::Insert => {
                self.requests.insert((text(change, 0)?, text(change, 1)?, text(change, 2)?));
                if self.requests.len() > 1 {
                    return Err(sql::invalid("one pending operation cannot contain unrelated requests"));
                }
            }
            "sync_pending_mutations"
                if change.action() == Action::Update && matches!(super::ACTIVE.get(), Some(super::Role::Pending)) =>
            {
                for column in [0, 1, 2, 3, 5, 6, 7, 8, 9] {
                    if change.before(column)? != change.after(column)? {
                        return Err(sql::invalid("local staging cannot rewrite a pending request or its state"));
                    }
                }
            }
            "sync_pending_objects" => {
                if change.action() != Action::Insert {
                    return Err(sql::invalid("a pending object's immutable intent cannot be rewritten"));
                }
                self.objects.insert((
                    text(change, 0)?,
                    text(change, 1)?,
                    text(change, 2)?,
                    text(change, 3)?,
                    text(change, 4)?,
                ));
                if self.objects.len() > 256 {
                    return Err(sql::invalid("pending operation exceeds the object audit budget"));
                }
            }
            "sync_local_changes" if change.action() == Action::Insert && change.after(5)? == ValueRef::Null => {
                let ValueRef::Blob(sequence) = change.after(0)? else {
                    return Err(sql::invalid("local change cursor is not a fixed-width sequence"));
                };
                self.notices.insert(
                    (text(change, 1)?, text(change, 2)?, text(change, 3)?, text(change, 8)?),
                    sequence.to_vec(),
                );
                if self.notices.len() > 256 {
                    return Err(sql::invalid("pending operation exceeds the notice audit budget"));
                }
            }
            "sync_outbox" if change.action() != Action::Delete && text(change, 0)? == "authority" => {
                let ValueRef::Blob(body) = change.after(4)? else {
                    return Err(sql::invalid("asset delivery intent is not encoded metadata"));
                };
                if body.len() > fastctx_types::sync::PAGE_BYTES as usize {
                    return Err(sql::invalid("asset delivery intent exceeds one page"));
                }
                self.outbound.insert((text(change, 1)?, text(change, 3)?), body.to_vec());
                if self.outbound.len() > 1 {
                    return Err(sql::invalid("one pending operation has one atomic outgoing request"));
                }
            }
            _ => {}
        }
        Ok(())
    }
    pub(super) fn validate(&self, db: &Connection) -> Result<()> {
        let groups = self
            .objects
            .iter()
            .map(|(hub, origin, operation, _, _)| (hub.clone(), origin.clone(), operation.clone()))
            .collect::<BTreeSet<_>>();
        if groups != self.requests {
            return Err(sql::invalid("pending request and object effects do not form one atomic change"));
        }
        for (hub, origin, operation) in &self.requests {
            let (body,hash)=db.prepare_cached("SELECT original_request,request_hash FROM sync_pending_mutations WHERE hub=?1 AND origin=?2 AND operation=?3")?.query_row(params![hub,origin,operation],|r|Ok((r.get::<_,Vec<u8>>(0)?,r.get::<_,String>(1)?)))?;
            if body.len() > fastctx_types::sync::PAGE_BYTES as usize {
                return Err(sql::invalid("persisted pending request exceeds its metadata budget"));
            }
            let request: Submission = serde_json::from_slice(&body).map_err(|e| sql::invalid(e.to_string()))?;
            request.validate_bounds().map_err(sql::invalid)?;
            if request.operation.as_str() != operation
                || request.origin.as_str() != origin
                || request.objects[0].key.scope.hub.to_string() != *hub
                || crate::store::engine::request_fingerprint(&request)
                    .map_err(|e| sql::invalid(e.to_string()))?
                    .as_str()
                    != hash
                || self.outbound.get(&(hub.clone(), operation.clone())) != Some(&body)
            {
                return Err(sql::invalid("pending request, fingerprint and outgoing intent do not agree"));
            }
            if request.objects.len() != self.objects.len() {
                return Err(sql::invalid("atomic pending request is missing an object effect"));
            }
            let outgoing:bool=db.prepare_cached("SELECT EXISTS(SELECT 1 FROM sync_outbox WHERE destination='authority' AND hub=?1 AND scope=?2 AND operation=?3 AND body=?4)")?.query_row(params![hub,request.objects[0].key.scope.scope.as_str(),operation,body],|r|r.get(0))?;
            if !outgoing {
                return Err(sql::invalid("atomic pending delivery intent disappeared before commit"));
            }
            for object in &request.objects {
                let scope = object.key.scope.scope.as_str();
                let id = object.key.object.as_str();
                if !self.objects.contains(&(hub.clone(), origin.clone(), operation.clone(), scope.into(), id.into()))
                    || !self.notices.contains_key(&(hub.clone(), scope.into(), id.into(), operation.clone()))
                {
                    return Err(sql::invalid("pending object has no new durable local change"));
                }
                let sequence = &self.notices[&(hub.clone(), scope.into(), id.into(), operation.clone())];
                let noticed:bool=db.prepare_cached("SELECT EXISTS(SELECT 1 FROM sync_local_changes WHERE sequence=?1 AND hub=?2 AND scope=?3 AND object=?4 AND kind=?5 AND operation=?6 AND authority_epoch IS NULL AND revision IS NULL)")?.query_row(params![sequence,hub,scope,id,object.kind,operation],|r|r.get(0))?;
                if !noticed {
                    return Err(sql::invalid("pending local change disappeared before commit"));
                }
                let (kind,base,before,candidate,predecessor,intent)=db.prepare_cached("SELECT kind,base,before_root,candidate_root,predecessor,original_intent FROM sync_pending_objects WHERE hub=?1 AND origin=?2 AND operation=?3 AND scope=?4 AND object=?5")?.query_row(params![hub,origin,operation,scope,id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,Vec<u8>>(1)?,r.get::<_,Option<Vec<u8>>>(2)?,r.get::<_,Option<Vec<u8>>>(3)?,r.get::<_,Option<String>>(4)?,r.get::<_,Vec<u8>>(5)?)))?;
                if kind != object.kind
                    || decode::<crate::store::mutation::BaseVersion>(&base)? != object.base
                    || before.as_deref().map(decode::<ContentRef>).transpose()? != object.before
                    || candidate.as_deref().map(decode::<ContentRef>).transpose()? != object.candidate
                    || predecessor.as_deref() != object.predecessor.as_ref().map(|id| id.as_str())
                    || decode::<crate::store::mutation::OriginalIntent>(&intent)? != object.original
                {
                    return Err(sql::invalid("pending object differs from its immutable request"));
                }
                let holding: Vec<u8> = db.prepare_cached("SELECT holding FROM sync_pending_objects WHERE hub=?1 AND origin=?2 AND operation=?3 AND scope=?4 AND object=?5")?.query_row(params![hub,origin,operation,scope,id],|r|r.get(0))?;
                let holding: crate::cursor::HoldingPeriod = decode(&holding)?;
                let publication = db.prepare_cached("SELECT publication_epoch FROM sync_scopes WHERE hub=?1 AND scope=?2 AND enabled=1 AND acl_revision=?3")?.query_row(params![hub,scope,object.observed_acl.0.to_be_bytes().as_slice()],|r|sql::counter(r,0))?;
                if holding.scope != object.key.scope
                    || holding.acl_revision != object.observed_acl
                    || holding.publication_epoch.0 != publication
                {
                    return Err(sql::invalid("pending object lacks its own current scope holding fence"));
                }
                let base = match &object.base {
                    crate::store::mutation::BaseVersion::Absent => None,
                    crate::store::mutation::BaseVersion::Version { root, .. } => root.as_ref(),
                };
                for (role, root) in
                    [("base", base), ("before", object.before.as_ref()), ("candidate", object.candidate.as_ref())]
                {
                    if let Some(root) = root {
                        let (form, length) = match root {
                            ContentRef::Single { decoded_bytes, .. } => ("block", Some(*decoded_bytes)),
                            ContentRef::Manifest { .. } => ("manifest", None),
                        };
                        let kept:bool=db.prepare_cached("SELECT EXISTS(SELECT 1 FROM sync_pending_roots WHERE hub=?1 AND origin=?2 AND operation=?3 AND scope=?4 AND object=?5 AND kind=?6 AND hash=?7 AND root_kind=?8 AND decoded_bytes IS ?9 AND role=?10)")?.query_row(params![hub,origin,operation,scope,id,object.kind,root.id().digest.as_str(),form,length,role],|r|r.get(0))?;
                        if !kept {
                            return Err(sql::invalid("pending edit would lose a declared body root"));
                        }
                    }
                }
            }
        }
        Ok(())
    }
    pub(super) fn clear(&mut self) {
        self.requests.clear();
        self.objects.clear();
        self.notices.clear();
        self.outbound.clear();
    }
}
fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    serde_json::from_slice(bytes).map_err(|e| sql::invalid(e.to_string()))
}
