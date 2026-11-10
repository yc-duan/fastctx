//! One bounded semantic unit and its local effects. The caller adopts these
//! effects only when the shared canonical producer consumed the complete unit.
use super::{
    bodies::Bodies, claim::Claim, display_append, indexes::Indexes, render, resupply, row_state::*,
    step_content::StepContent,
};
use fastctx_hosts::claude::ingest::{
    checkpoint::{ClaudeCheckpointV1, Partial},
    document::{self, read::Reader},
    jsonl::Row,
    semantic::row::RowKind,
};
use fastctx_sync::{
    cursor::{ByteCount, Sequence},
    identity::ObjectId,
    manifest::ContentRef,
    store::{
        Error,
        ingest::SourceIngest,
        ingest_effects::{SourceFamily, SourceIndexChange},
    },
};
use fastctx_types::sync::{
    conversation_identity::{IdentityError, ItemRole, NativeId, ProvisionalKey},
    conversation_timeline::*,
};

pub struct Unit {
    pub fact: Option<TimelineFact>,
    pub accepted: Option<Sequence>,
    pub effects: Vec<SourceIndexChange>,
    pub finished: bool,
}
pub async fn unit(
    source: &dyn SourceIngest,
    claim: &Claim,
    io: &StepContent<'_>,
    checkpoint: &ClaudeCheckpointV1,
) -> Result<Unit, Error> {
    let Some(Partial::Publishing { row, .. }) = &checkpoint.partial else {
        return Err(Error::Invalid("publisher needs a validated pending row".into()));
    };
    let key = format!("claude/consumer/{}", claim.primary.source.as_str());
    let mut rows = Indexes::new(source, &claim.canonical);
    let mut work = rows.get::<Work>(SourceFamily::Consumers, &key).await?.unwrap_or_else(|| Work::new(row.start));
    if work.start != row.start {
        return Err(Error::Corrupt("consumer belongs to a different source row".into()));
    }
    let mut clock = rows.get::<Clock>(SourceFamily::HostRows, CLOCK).await?.unwrap_or_default();
    let fact = match &row.content {
        document::Completion::Valid(document) => {
            let input = Bodies { content: io };
            let reader = Reader { document, input: &input };
            match fastctx_hosts::claude::ingest::semantic::row::header(&reader, None)
                .await
                .map_err(super::reading::error)?
            {
                RowKind::Skipped | RowKind::Title { .. } => {
                    work.stage = Stage::Finish;
                    None
                }
                RowKind::Row(header)
                    if header.problem.is_none()
                        && header.uuid.as_deref().is_some_and(|id| NativeId::new(id).is_ok()) =>
                {
                    super::row_index::advance(
                        claim,
                        io,
                        &reader,
                        &header,
                        row,
                        Local::borrow(&mut rows, &mut clock, &mut work),
                    )
                    .await?
                }
                _ => {
                    work.stage = Stage::Finish;
                    Some(unknown(claim, io, row, Local::borrow(&mut rows, &mut clock, &mut work)).await?)
                }
            }
        }
        document::Completion::Invalid { .. } => {
            work.stage = Stage::Finish;
            Some(unknown(claim, io, row, Local::borrow(&mut rows, &mut clock, &mut work)).await?)
        }
    };
    let accepted = if fact.is_some() {
        let accepted = clock.accept()?;
        rows.put(SourceFamily::HostRows, CLOCK, &clock).await?;
        Some(accepted)
    } else {
        None
    };
    let finished = matches!(work.stage, Stage::Finish);
    if finished {
        rows.delete(SourceFamily::Consumers, &key).await?;
    } else {
        rows.put(SourceFamily::Consumers, &key, &work).await?;
    }
    Ok(Unit { fact, accepted, effects: rows.effects(), finished })
}
pub(super) fn block_key(claim: &Claim, row: &Row, index: u64) -> String {
    format!("claude/block/{}/{:016x}/{index:016x}", claim.primary.source.as_str(), row.start.0)
}
pub(super) fn identity(e: IdentityError) -> Error {
    Error::Invalid(e.to_string())
}
pub(super) fn original(row: &Row) -> &ContentRef {
    match &row.content {
        document::Completion::Valid(doc) => &doc.bodies.raw.content,
        document::Completion::Invalid { bodies, .. } => &bodies.raw.content,
    }
}
pub(super) fn item_key(id: &ObjectId) -> String {
    format!("claude/item/{}", id.as_str())
}
pub(super) async fn observed(
    io: &StepContent<'_>,
    row: &Row,
    local: Local<'_, '_>,
    identity: ItemIdentity,
    display: ItemDisplayV1,
    complete: bool,
) -> Result<TimelineFact, Error> {
    let Local { rows, clock, work } = local;
    let key = item_key(&identity.id());
    let old = rows.get::<Item>(SourceFamily::HostRows, &key).await?;
    let created = match old.as_ref() {
        Some(old) => old.created,
        None => clock.create()?,
    };
    let placement = old.as_ref().and_then(|old| old.placement.clone()).or_else(|| work.placement.clone());
    if !work.original_published {
        resupply::state_content(io, original(row)).await?;
        work.original_published = true;
    }
    let display = display_append::publish(io, &display).await?;
    rows.put(SourceFamily::HostRows, &key, &Item { created, placement: placement.clone(), display: display.clone() })
        .await?;
    Ok(TimelineFact::Observed {
        observation: Observation {
            identity,
            created,
            authority: if complete { BodyAuthority::HostFinal } else { BodyAuthority::HostDraft },
            body: BodySnapshot {
                original: original(row).clone(),
                display,
                complete,
                text_part: None,
                text_bytes: ByteCount(0),
            },
            placement,
        },
    })
}
pub(super) async fn unknown(
    claim: &Claim,
    io: &StepContent<'_>,
    row: &Row,
    local: Local<'_, '_>,
) -> Result<TimelineFact, Error> {
    let Local { rows, clock, work } = local;
    let identity = ItemIdentity::Provisional(ProvisionalKey {
        conversation: claim.conversation.clone(),
        role: ItemRole::System,
        source: claim.primary.source.clone(),
        activation: NativeId::new(io.target().step().generation().as_str()).map_err(identity)?,
        draft: NativeId::new(row.start.0.to_string()).map_err(identity)?,
    });
    observed(
        io,
        row,
        Local::borrow(rows, clock, work),
        identity,
        render::empty(DisplayKind::Unknown, None, DisplayStatus::Failed),
        true,
    )
    .await
}
