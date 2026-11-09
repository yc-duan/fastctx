//! Bounded out-of-order evidence stays durable until its missing predecessor arrives.
use super::{SourceAccepted, SqliteAuthority, decode, encode, prepare::Verified, streams};
use crate::{
    catalog::SourceBatch,
    cursor::{HoldingPeriod, LocalSequence},
    identity::{AccountId, DeviceId, ProducerEpoch, ScopeKey, StreamId},
    proof::{Bytes, ProofPurpose, Signature, SignedSourceBatch, SignedStatement, SigningKey},
    scheduler::{Lease, Outcome, TaskHandler, TaskKey},
    store::{
        Error, FutureResult,
        access::{AccessContext, Principal},
        local_head, sql,
    },
};
use fastctx_storage::{
    Result,
    rusqlite::{Connection, OptionalExtension, params},
};
const PREFIX: &str = "source-accept/";

pub(super) fn dirty(db: &Connection, batch: &SourceBatch, local: LocalSequence) -> Result<()> {
    crate::scheduler::projection_work(
        db,
        &batch.scope,
        format!("{PREFIX}{}/{}", batch.stream.as_str(), batch.epoch.as_str()),
        local,
    )
}
pub(super) fn save(db: &Connection, authority: &SqliteAuthority, verified: &Verified) -> Result<()> {
    let b = &verified.batch;
    let Principal::Account { account, device } = &verified.bound.source.context.principal else {
        return Err(sql::invalid("waiting source lost its authenticated identity"));
    };
    let holding = encode(&verified.bound.source.holding)?;
    let bytes = verified.signed.proof.data.0.len() + holding.len() + 1024;
    let previous: Option<(i64,String,String,Vec<u8>)> = db.prepare_cached("SELECT bytes,account,device,holding FROM sync_authority_waiting WHERE hub=?1 AND scope=?2 AND stream=?3 AND epoch=?4 AND operation=?5")?
        .query_row(params![b.scope.hub.to_string(),b.scope.scope.as_str(),b.stream.as_str(),b.epoch.as_str(),b.operation.as_str()],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
    if previous.as_ref().is_some_and(|(_, a, d, h)| a == account.as_str() && d == device.as_str() && h == &holding) {
        return Ok(());
    }
    let used: i64 = db
        .prepare_cached("SELECT bytes FROM sync_authority_waiting_usage WHERE hub=?1")?
        .query_row(params![b.scope.hub.to_string()], |r| r.get(0))
        .optional()?
        .unwrap_or(0);
    let next = used - previous.map_or(0, |(bytes, _, _, _)| bytes) + bytes as i64;
    if next > authority.awaiting_bytes as i64 {
        return Err(fastctx_storage::Error::with_kind(
            fastctx_storage::ErrorKind::Busy,
            "awaiting source metadata budget is full",
        ));
    }
    db.prepare_cached("INSERT INTO sync_authority_waiting_usage(hub,bytes) VALUES(?1,?2) ON CONFLICT(hub) DO UPDATE SET bytes=excluded.bytes")?.execute(params![b.scope.hub.to_string(),next])?;
    streams::write_proof(db, b, &verified.signed, "awaiting_epoch")?;
    db.prepare_cached("INSERT INTO sync_authority_waiting(hub,scope,stream,epoch,operation,after_sequence,account,device,holding,bytes) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10) ON CONFLICT(hub,scope,stream,epoch,operation) DO UPDATE SET account=excluded.account,device=excluded.device,holding=excluded.holding,bytes=excluded.bytes")?
        .execute(params![b.scope.hub.to_string(),b.scope.scope.as_str(),b.stream.as_str(),b.epoch.as_str(),b.operation.as_str(),b.after.0.to_be_bytes().as_slice(),account.as_str(),device.as_str(),holding,bytes as i64])?;
    dirty(db, b, local_head::advance(db)?)
}
pub(super) fn remove(db: &Connection, batch: &SourceBatch) -> Result<()> {
    let bytes:Option<i64>=db.prepare_cached("SELECT bytes FROM sync_authority_waiting WHERE hub=?1 AND scope=?2 AND stream=?3 AND epoch=?4 AND operation=?5")?
        .query_row(params![batch.scope.hub.to_string(),batch.scope.scope.as_str(),batch.stream.as_str(),batch.epoch.as_str(),batch.operation.as_str()],|r|r.get(0)).optional()?;
    if let Some(bytes) = bytes {
        db.prepare_cached("UPDATE sync_authority_waiting_usage SET bytes=bytes-?2 WHERE hub=?1")?
            .execute(params![batch.scope.hub.to_string(), bytes])?;
    }
    db.prepare_cached(
        "DELETE FROM sync_authority_waiting WHERE hub=?1 AND scope=?2 AND stream=?3 AND epoch=?4 AND operation=?5",
    )?
    .execute(params![
        batch.scope.hub.to_string(),
        batch.scope.scope.as_str(),
        batch.stream.as_str(),
        batch.epoch.as_str(),
        batch.operation.as_str()
    ])?;
    Ok(())
}
struct Waiting {
    signed: SignedSourceBatch,
    context: AccessContext,
    holding: HoldingPeriod,
}
fn next(db: &Connection, scope: &ScopeKey, stream: &StreamId, epoch: &ProducerEpoch) -> Result<Option<Waiting>> {
    let row=db.prepare_cached("SELECT w.account,w.device,w.holding,p.purpose,p.signer,p.signed_bytes,p.signature FROM sync_authority_waiting w JOIN sync_source_proofs p ON p.hub=w.hub AND p.scope=w.scope AND p.stream=w.stream AND p.epoch=w.epoch AND p.operation=w.operation WHERE w.hub=?1 AND w.scope=?2 AND w.stream=?3 AND w.epoch=?4 ORDER BY w.after_sequence,w.operation LIMIT 1")?
        .query_row(params![scope.hub.to_string(),scope.scope.as_str(),stream.as_str(),epoch.as_str()],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,Vec<u8>>(2)?,r.get::<_,String>(3)?,r.get::<_,Vec<u8>>(4)?,r.get::<_,Vec<u8>>(5)?,r.get::<_,Vec<u8>>(6)?))).optional()?;
    row.map(|(account, device, holding, purpose, key, data, signature)| {
        let holding: HoldingPeriod = decode(&holding)?;
        Ok(Waiting {
            context: AccessContext {
                principal: Principal::Account {
                    account: AccountId::try_from(account).map_err(sql::invalid)?,
                    device: DeviceId::try_from(device).map_err(sql::invalid)?,
                },
                scope: scope.clone(),
                observed_acl: holding.acl_revision,
            },
            holding,
            signed: SignedSourceBatch {
                proof: SignedStatement {
                    purpose: ProofPurpose::from_domain(&purpose)
                        .ok_or_else(|| sql::invalid("unknown waiting proof purpose"))?,
                    signer: SigningKey(key.try_into().map_err(|_| sql::invalid("invalid waiting signer"))?),
                    data: Bytes(data),
                    signature: Signature(signature.try_into().map_err(|_| sql::invalid("invalid waiting signature"))?),
                },
            },
        })
    })
    .transpose()
}
pub(super) async fn process(authority: &SqliteAuthority, lease: Lease) -> std::result::Result<Outcome, Error> {
    let rest =
        lease.task.key.key.strip_prefix(PREFIX).ok_or_else(|| Error::Invalid("not an authority source task".into()))?;
    let (stream, epoch) = rest.split_once('/').ok_or_else(|| Error::Invalid("invalid authority source task".into()))?;
    let stream = StreamId::try_from(stream.to_owned()).map_err(Error::Invalid)?;
    let epoch = ProducerEpoch::try_from(epoch.to_owned()).map_err(Error::Invalid)?;
    let scope = lease.task.key.scope.clone();
    let waiting = authority
        .blocks
        .database()
        .read(move |db| next(db, &scope, &stream, &epoch))
        .await
        .map_err(|e| sql::error(e, authority.blocks.worker().clone()))?;
    let Some(waiting) = waiting else {
        return Ok(Outcome::Completed { through: lease.task.wanted });
    };
    let signed = waiting.signed.clone();
    let result = authority.accept(waiting.context, waiting.holding, waiting.signed).await;
    if result.is_err() && super::rejected::overlap(authority, signed).await? {
        return Ok(Outcome::Retry { next_due_millis: 0, reason: "retired an overlapping waiting source batch".into() });
    }
    match result? {
        SourceAccepted::Accepted { .. } => {
            Ok(Outcome::Retry { next_due_millis: 0, reason: "checking the next waiting source batch".into() })
        }
        SourceAccepted::AwaitingEpoch { .. } => Ok(Outcome::Blocked {
            dependency: format!("{}:gap", lease.task.key.key),
            reason: "waiting for the preceding source interval".into(),
        }),
    }
}
impl TaskHandler for SqliteAuthority {
    fn kind(&self) -> &'static str {
        "sync.projection"
    }
    fn takes(&self, key: &TaskKey) -> bool {
        key.kind == self.kind() && key.key.starts_with(PREFIX)
    }
    fn handle(&self, lease: Lease) -> FutureResult<'_, Outcome> {
        self.process(lease)
    }
}
