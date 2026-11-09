//! One FULL transaction publishes facts, epoch evidence, stream and feed heads,
//! references and the immutable scoped receipt. Refusals cannot publish half a batch.
use super::{SourceAccepted, SqliteAuthority, access, prepare, receipts, streams, waiting};
use crate::{
    cursor::HoldingPeriod,
    proof::SignedSourceBatch,
    store::{
        Error,
        access::AccessContext,
        boundary::{self, Role},
        sql,
    },
};
use fastctx_storage::{
    Result,
    rusqlite::{Connection, params},
};

enum Admission {
    Ready,
    Waiting,
    Conflict,
}
pub(super) async fn accept(
    authority: &SqliteAuthority,
    context: AccessContext,
    holding: HoldingPeriod,
    signed: SignedSourceBatch,
) -> std::result::Result<SourceAccepted, Error> {
    let verified = prepare::verify(authority, context, holding, signed)?;
    let op = verified.batch.operation.clone();
    let id = verified.signed.proof.id();
    let checked = prepare::Verified {
        batch: verified.batch.clone(),
        signed: verified.signed.clone(),
        bound: access::Bound {
            descriptor: verified.bound.descriptor.clone(),
            source: verified.bound.source.clone(),
            hub: verified.bound.hub.clone(),
        },
    };
    let service = authority.clone();
    let admitted = authority
        .blocks
        .database()
        .write(fastctx_types::sync::PAGE_BYTES as usize * 2, move |db| {
            let _boundary = boundary::enter_typed_write(Role::Authority);
            current(db, &service, &checked)?;
            if receipts::conflict(db, &checked)? {
                return Ok(Admission::Conflict);
            }
            if receipts::get(db, &checked)?.is_some() {
                return Ok(Admission::Ready);
            }
            let sequence = streams::head(db, &checked.batch)?.map_or(0, |h| h.sequence.0);
            if checked.batch.after.0 < sequence {
                return Err(sql::invalid("source batch overlaps an already accepted interval"));
            }
            if checked.batch.after.0 > sequence {
                waiting::save(db, &service, &checked)?;
                return Ok(Admission::Waiting);
            }
            Ok(Admission::Ready)
        })
        .await
        .map_err(|e| sql::error(e, op.clone()))?;
    match admitted {
        Admission::Conflict => {
            return Err(Error::Invalid(
                "source operation was reused with different original evidence; conflict retained".into(),
            ));
        }
        Admission::Waiting => return Ok(SourceAccepted::AwaitingEpoch { proof: id }),
        Admission::Ready => {}
    }
    let prepared = prepare::prepare(authority, verified).await?;
    let complete = prepared.complete;
    let service = authority.clone();
    let result = authority
        .blocks
        .database()
        .write(fastctx_types::sync::PAGE_BYTES as usize * 4, move |db| {
            let _boundary = boundary::enter_typed_write(Role::Authority);
            let checked = prepare::Verified {
                batch: prepared.batch.clone(),
                signed: prepared.signed.clone(),
                bound: access::Bound {
                    descriptor: prepared.bound.descriptor.clone(),
                    source: prepared.bound.source.clone(),
                    hub: prepared.bound.hub.clone(),
                },
            };
            current(db, &service, &checked)?;
            if receipts::conflict(db, &checked)? {
                return Ok(None);
            }
            if let Err(error) = prepare::authorize(&service, &prepared) {
                return Ok(Some(Err(error)));
            }
            let receipt = if let Some(previous) = receipts::get(db, &checked)? {
                previous
            } else {
                let sequence = streams::head(db, &prepared.batch)?.map_or(0, |h| h.sequence.0);
                if sequence != prepared.batch.after.0 {
                    return Err(fastctx_storage::Error::with_kind(
                        fastctx_storage::ErrorKind::Busy,
                        "source head moved while preparing",
                    ));
                }
                let receipt = streams::append(db, &service, &prepared)?;
                receipts::save(db, &prepared.batch, &receipt)?;
                waiting::remove(db, &prepared.batch)?;
                receipt
            };
            for root in &prepared.ready {
                prepared.supplies.adopt(db, &service.blocks, &prepared.bound.hub, root)?;
            }
            if prepared.complete {
                crate::blocks::projection_commit::close_staging(db, &prepared.bound.hub, &prepared.batch.operation)?;
            }
            Ok(Some(Ok(receipt)))
        })
        .await
        .map_err(|e| sql::error(e, op))?;
    match result {
        Some(result) => {
            result.map(|receipt| SourceAccepted::Accepted { receipt: Box::new(receipt), content_complete: complete })
        }
        None => Err(Error::Invalid(
            "source operation was reused with different original evidence; conflict retained".into(),
        )),
    }
}
fn current(db: &Connection, authority: &SqliteAuthority, verified: &prepare::Verified) -> Result<()> {
    let active: Vec<u8> = db
        .prepare_cached("SELECT authority_epoch FROM sync_hub_control_heads WHERE hub=?1")?
        .query_row(params![authority.run.hub().to_string()], |r| r.get(0))?;
    if active != authority.run.epoch().bytes() {
        return Err(sql::invalid("this authority run was superseded"));
    }
    let source = &verified.bound.source;
    let found = access::bind(authority, db, &source.context, &source.holding, source.kind, &verified.batch.epoch)?;
    if found.descriptor != verified.bound.descriptor
        || found.hub.holding != verified.bound.hub.holding
        || found.hub.object != verified.bound.hub.object
    {
        return Err(fastctx_storage::Error::with_kind(
            fastctx_storage::ErrorKind::Busy,
            "source scope changed during preparation",
        ));
    }
    Ok(())
}
