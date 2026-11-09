//! Adoption and private protection are checked at the enclosing savepoint boundary.
use super::model::{Request, SupplyOwner, decode, encode};
use crate::{
    blocks::pins,
    store::{roots::RootUse, sql},
};
use fastctx_storage::{
    Result,
    guard::{Action, Change},
    rusqlite::{Connection, params, types::ValueRef},
};
use std::collections::BTreeSet;

type Key = (String, String, String, Vec<u8>);
#[derive(Default)]
pub(crate) struct SupplyAudit {
    consumed: BTreeSet<Key>,
    protected: BTreeSet<Key>,
    retired: BTreeSet<Key>,
    pins: super::pin_audit::PinAudit,
}
impl SupplyAudit {
    pub(crate) fn changed(&mut self, change: &Change<'_>) -> Result<()> {
        self.pins.changed(change)?;
        match change.table() {
            "sync_supply_checks" => {
                if change.action() == Action::Update {
                    for column in 0..=7 {
                        if change.before(column)? != change.after(column)? {
                            return Err(sql::invalid("supply work cannot change its immutable request identity"));
                        }
                    }
                }
                if change.action() != Action::Delete
                    && change.after(9)? == ValueRef::Text(b"consumed")
                    && (change.action() == Action::Insert || change.before(9)? != ValueRef::Text(b"consumed"))
                {
                    self.consumed.insert((
                        text(change.after(0)?)?,
                        text(change.after(1)?)?,
                        text(change.after(2)?)?,
                        blob(change.after(3)?)?,
                    ));
                }
            }
            "sync_root_leases" => {
                if change.action() != Action::Insert && change.before(15)? != ValueRef::Null {
                    let retired = change.action() == Action::Delete
                        || change.after(14)? != ValueRef::Integer(0)
                        || change.before(11)? != change.after(11)?
                        || change.before(12)? != change.after(12)?
                        || change.before(15)? != change.after(15)?
                        || change.before(16)? != change.after(16)?;
                    if retired {
                        self.retired.insert((
                            text(change.before(0)?)?,
                            text(change.before(1)?)?,
                            text(change.before(15)?)?,
                            blob(change.before(16)?)?,
                        ));
                    }
                }
                if change.action() != Action::Delete
                    && change.after(15)? != ValueRef::Null
                    && change.after(14)? == ValueRef::Integer(0)
                {
                    self.protected.insert((
                        text(change.after(0)?)?,
                        text(change.after(1)?)?,
                        text(change.after(15)?)?,
                        blob(change.after(16)?)?,
                    ));
                }
            }
            _ => {}
        }
        if self.consumed.len() + self.protected.len() + self.retired.len() > 4097 {
            return Err(sql::invalid("supply audit exceeds its operation bound"));
        }
        Ok(())
    }
    pub(crate) fn validate(&self, db: &Connection) -> Result<()> {
        self.pins.validate(db)?;
        for (hub, scope, id, generation) in &self.protected {
            let exists: bool = db.query_row(
                "SELECT EXISTS(SELECT 1 FROM sync_supply_checks WHERE hub=?1 AND scope=?2 AND id=?3 AND generation=?4)",
                params![hub, scope, id, generation],
                |row| row.get(0),
            )?;
            if !exists {
                return Err(sql::invalid("private supply protection has no owning check"));
            }
        }
        for (hub, scope, id, generation) in &self.retired {
            let terminal:bool=db.query_row(
                "SELECT EXISTS(SELECT 1 FROM sync_supply_checks WHERE hub=?1 AND scope=?2 AND id=?3 AND generation=?4 AND phase IN ('paused','rejected','cancelled','consumed'))",
                params![hub,scope,id,generation],|row|row.get(0),
            )?;
            if !terminal {
                return Err(sql::invalid("supply protection retired before invalidating its check"));
            }
        }
        for (hub, scope, id, generation) in &self.consumed {
            let body: Vec<u8> = db.query_row(
                "SELECT request FROM sync_supply_checks WHERE hub=?1 AND scope=?2 AND id=?3 AND generation=?4",
                params![hub, scope, id, generation],
                |row| row.get(0),
            )?;
            let request: Request = decode(&body)?;
            if !retained(db, &request)? {
                return Err(sql::invalid(
                    "supplied content was adopted without its durable owner in the same operation",
                ));
            }
        }
        Ok(())
    }
    pub(crate) fn clear(&mut self) {
        self.consumed.clear();
        self.protected.clear();
        self.retired.clear();
        self.pins.clear();
    }
}

pub(super) fn retained(db: &Connection, request: &Request) -> Result<bool> {
    let object = &request.object;
    let hash = request.root.id().digest.as_str();
    let (form, length) = pins::shape(&request.root);
    match &request.owner {
        SupplyOwner::Use {usage} => retained_use(db,request,usage),
        SupplyOwner::Mutation => db.query_row(
            "SELECT EXISTS(SELECT 1 FROM sync_pending_roots WHERE hub=?1 AND scope=?2 AND object=?3 AND kind=?4 AND operation=?5 AND hash=?6 AND root_kind=?7 AND decoded_bytes IS ?8) OR EXISTS(SELECT 1 FROM sync_object_versions v JOIN sync_objects o USING(hub,scope,object) JOIN sync_asset_roots r USING(hub,scope,object,authority_epoch,revision) WHERE v.hub=?1 AND v.scope=?2 AND v.object=?3 AND o.kind=?4 AND v.operation=?5 AND r.hash=?6 AND r.root_kind=?7 AND r.decoded_bytes IS ?8)",
            params![object.scope.hub.to_string(),object.scope.scope.as_str(),object.object.as_str(),request.kind,request.operation.as_str(),hash,form,length],|row|row.get(0),
        ).map_err(Into::into),
        SupplyOwner::Source {source} => {
            if retained_use(db,request,&RootUse::Checkpoint {source:source.clone()})? { return Ok(true); }
            db.query_row(
                "SELECT EXISTS(SELECT 1 FROM sync_asset_uploads u JOIN sync_source_step_targets t ON t.hub=u.hub AND t.scope=u.source_scope AND t.source=u.source AND t.step=u.source_step AND t.target_scope=u.scope AND t.object=u.object AND t.kind=u.kind JOIN sync_source_proofs p ON p.hub=t.hub AND p.scope=t.target_scope AND p.stream=t.stream AND p.operation=u.operation JOIN sync_stream_roots r ON r.hub=p.hub AND r.scope=p.scope AND r.stream=p.stream AND r.epoch=p.epoch AND r.sequence BETWEEN p.first_sequence AND p.last_sequence JOIN sync_stream_records f ON f.hub=r.hub AND f.scope=r.scope AND f.stream=r.stream AND f.epoch=r.epoch AND f.sequence=r.sequence AND f.batch_proof=p.proof WHERE u.hub=?1 AND u.scope=?2 AND u.object=?3 AND u.kind=?4 AND u.operation=?5 AND t.target_source=?6 AND r.hash=?7 AND r.root_kind=?8 AND r.decoded_bytes IS ?9 AND f.kind=u.kind)",
                params![object.scope.hub.to_string(),object.scope.scope.as_str(),object.object.as_str(),request.kind,request.operation.as_str(),source.as_str(),hash,form,length],|row|row.get(0),
            ).map_err(Into::into)
        }
    }
}
fn retained_use(db: &Connection, request: &Request, usage: &RootUse) -> Result<bool> {
    let object = &request.object;
    let (form, length) = pins::shape(&request.root);
    db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sync_root_leases WHERE hub=?1 AND scope=?2 AND object=?3 AND kind=?4 AND operation=?5 AND hash=?6 AND root_kind=?7 AND decoded_bytes IS ?8 AND usage=?9 AND holding=?10 AND purpose=?11 AND released=0 AND supply_id IS NULL)",
        params![object.scope.hub.to_string(),object.scope.scope.as_str(),object.object.as_str(),request.kind,request.operation.as_str(),request.root.id().digest.as_str(),form,length,encode(usage)?,encode(&request.holding)?,encode(&request.purpose)?],|row|row.get(0),
    ).map_err(Into::into)
}
fn text(value: ValueRef<'_>) -> Result<String> {
    match value {
        ValueRef::Text(bytes) => String::from_utf8(bytes.to_vec()).map_err(|e| sql::invalid(e.to_string())),
        _ => Err(sql::invalid("invalid supply audit text key")),
    }
}
fn blob(value: ValueRef<'_>) -> Result<Vec<u8>> {
    match value {
        ValueRef::Blob(bytes) if bytes.len() == 8 => Ok(bytes.to_vec()),
        _ => Err(sql::invalid("invalid supply audit generation")),
    }
}
