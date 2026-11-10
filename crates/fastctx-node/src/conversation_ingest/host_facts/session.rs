//! Conversation facts use a turn-free channel and retain stable revision keys.
use super::{content, context, id_list::IdList, row_stats, usage::Usage};
use crate::conversation_ingest::{
    claim::Claim,
    indexes::{self, Indexes},
    publisher, render,
    row_state::Local,
    step_content::StepContent,
};
use fastctx_hosts::claude::ingest::{
    document::read::Reader,
    jsonl::Row,
    semantic::{
        fields, message,
        row::{Header, RowKind},
    },
};
use fastctx_sync::store::{Error, ingest_effects::SourceFamily};
use fastctx_types::sync::{
    conversation_identity::{ItemRole, NativeId, ProvisionalKey},
    conversation_timeline::*,
    cursor::{ByteCount, Sequence},
};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct Pending {
    start: ByteCount,
    stage: u8,
    list: Option<IdList>,
    usage: Option<Usage>,
}
pub enum Advance {
    More(Option<Box<TimelineFact>>),
    Done,
}
fn key(claim: &Claim) -> String {
    format!("claude/public-meta/{}", claim.primary.source.as_str())
}
pub async fn clear(claim: &Claim, io: &StepContent<'_>, rows: &mut Indexes<'_>) -> Result<(), Error> {
    row_stats::clear(io, rows).await?;
    if rows.get::<Pending>(SourceFamily::Consumers, &key(claim)).await?.is_some() {
        rows.delete(SourceFamily::Consumers, &key(claim)).await?;
    }
    Ok(())
}
pub async fn advance(
    claim: &Claim,
    io: &StepContent<'_>,
    reader: &Reader<'_>,
    kind: &RowKind,
    row: &Row,
    local: Local<'_, '_>,
) -> Result<Advance, Error> {
    let Local { rows, clock, work } = local;
    let key = key(claim);
    let old = rows.get::<Pending>(SourceFamily::Consumers, &key).await?;
    let existed = old.is_some();
    let mut pending = old.unwrap_or(Pending { start: row.start, stage: 0, list: None, usage: None });
    if pending.start != row.start {
        return Err(super::invalid("public metadata belongs to another source row"));
    }
    if pending.stage == 4 {
        return Ok(Advance::Done);
    }
    let stats = match kind {
        RowKind::Row(header)
            if header.problem.is_none() && header.uuid.as_deref().is_some_and(|uuid| NativeId::new(uuid).is_ok()) =>
        {
            Some(row_stats::get(io, reader, header, row, rows).await?)
        }
        _ => None,
    };
    let event = event(io, reader, kind, stats.as_ref(), &mut pending).await;
    let (suffix, mut event, complete) = match event {
        Ok(Some(value)) => value,
        Ok(None) => {
            if existed || pending.stage != 4 {
                rows.put(SourceFamily::Consumers, &key, &pending).await?;
            }
            return Ok(if pending.stage == 4 { Advance::Done } else { Advance::More(None) });
        }
        Err(Error::Invalid(problem)) => {
            // Malformed metadata cannot make otherwise readable messages disappear.
            pending.stage = pending.stage.saturating_add(1).min(4);
            pending.list = None;
            pending.usage = None;
            (
                format!("diagnostic:{}", pending.stage),
                SessionFactV1::Diagnostic {
                    message: content::literal(io, &problem).await?,
                    detail: Some(
                        content::raw(io, reader, reader.node(reader.document.root).await.map_err(super::error)?.raw)
                            .await?,
                    ),
                },
                true,
            )
        }
        Err(error) => return Err(error),
    };
    if let RowKind::Row(header) = kind {
        context::link_compaction(rows, header, &mut event).await?;
    }
    if complete
        && let SessionFactV1::ContextRow { row: reference } = &event
        && let RowKind::Row(header) = kind
        && let Some(uuid) = &header.uuid
    {
        rows.put(SourceFamily::HostRows, &indexes::key("public-context", uuid), reference).await?;
        context::remember(rows, header).await?;
        rows.put(
            SourceFamily::HostRows,
            &indexes::key("response", uuid),
            &header
                .response
                .as_deref()
                .filter(|id| !id.is_empty())
                .map(|id| fastctx_types::sync::identity::Digest::of(id.as_bytes())),
        )
        .await?;
    }
    let at = match kind {
        RowKind::Row(header) => header.at,
        RowKind::Title { at, .. } => *at,
        _ => None,
    };
    let mut display = render::empty(
        match event {
            SessionFactV1::Compaction { .. } => DisplayKind::Compaction,
            SessionFactV1::Hidden { .. } => DisplayKind::Hidden,
            _ => DisplayKind::System,
        },
        None,
        if complete { DisplayStatus::Complete } else { DisplayStatus::Pending },
    );
    display.facts = Some(Box::new(DisplayFactsV1 {
        at,
        started_at: at,
        event: Some(content::publish(io, ItemFactV1::Session { event }).await?),
        ..Default::default()
    }));
    let native = match kind {
        RowKind::Row(header) => header.uuid.as_deref(),
        _ => None,
    };
    let suffix = if suffix.starts_with("title:") {
        suffix
    } else {
        format!("{suffix}:{}", native.map_or_else(|| row.start.0.to_string(), |uuid| indexes::key("native", uuid)))
    };
    let identity = ItemIdentity::Provisional(ProvisionalKey {
        conversation: claim.conversation.clone(),
        role: ItemRole::System,
        source: claim.canonical.source.clone(),
        // This names derived metadata, not an engine draft or a native turn.
        activation: NativeId::new("claude-jsonl-metadata-v1").map_err(super::invalid)?,
        draft: NativeId::new(suffix).map_err(super::invalid)?,
    });
    if matches!(kind, RowKind::Title { .. }) {
        display.facts.as_mut().ok_or(Error::Busy)?.native = Some(
            content::publish(
                io,
                ItemFactV1::Native {
                    identity: identity.clone(),
                    draft: None,
                    source: io.target().step().primary().source.clone(),
                    generation: io.target().step().generation().clone(),
                    line: Some(fastctx_types::sync::cursor::Count(row.line.0.checked_add(1).ok_or(Error::Busy)?)),
                    block: None,
                    response: None,
                    context: None,
                },
            )
            .await?,
        );
    }
    let previous = work.placement.take();
    work.placement = Some(Placement {
        turn: None,
        slot: DisplaySlot::Session,
        proof: PlacementProof::Causal,
        order: Some(DisplayOrder::Native { sequence: Sequence(row.line.0.checked_add(1).ok_or(Error::Busy)?) }),
    });
    let fact = publisher::observed(io, row, Local::borrow(rows, clock, work), identity, display, complete).await?;
    work.placement = previous;
    rows.put(SourceFamily::Consumers, &key, &pending).await?;
    Ok(Advance::More(Some(Box::new(fact))))
}
async fn event(
    io: &StepContent<'_>,
    reader: &Reader<'_>,
    kind: &RowKind,
    stats: Option<&row_stats::Stats>,
    pending: &mut Pending,
) -> Result<Option<(String, SessionFactV1, bool)>, Error> {
    match kind {
        RowKind::Title { custom, text: Some(text), .. } => {
            pending.stage = 4;
            return Ok(Some((
                format!("title:{}", if *custom { "custom" } else { "automatic" }),
                SessionFactV1::Title { custom: *custom, title: Some(content::text(io, reader, Some(*text)).await?) },
                true,
            )));
        }
        RowKind::Title { text: None, .. } => {
            pending.stage = 3;
            Err(super::invalid("native title is not a string"))
        }
        RowKind::Row(header)
            if header.problem.is_none() && header.uuid.as_deref().is_some_and(|uuid| NativeId::new(uuid).is_ok()) =>
        {
            row_event(io, reader, header, stats.ok_or(Error::Busy)?, pending).await
        }
        _ => {
            pending.stage = 4;
            Ok(None)
        }
    }
}
async fn row_event(
    io: &StepContent<'_>,
    reader: &Reader<'_>,
    header: &Header,
    stats: &row_stats::Stats,
    pending: &mut Pending,
) -> Result<Option<(String, SessionFactV1, bool)>, Error> {
    let root = reader.document.root;
    match pending.stage {
        0 => {
            let (supersedes, complete) = if let Some(at) = header.supersedes {
                let list = match pending.list.as_mut() {
                    Some(list) => list,
                    None => pending.list.insert(IdList::new(reader, at).await?),
                };
                let (reference, complete) = list.advance(io, reader).await?;
                (Some(reference), complete)
            } else {
                (None, true)
            };
            let reference = context::native_row(io, reader, header, supersedes).await?;
            if complete {
                pending.stage = 1;
                pending.list = None;
            }
            Ok(Some(("context".into(), SessionFactV1::ContextRow { row: reference }, complete)))
        }
        1 => {
            let observed = message::decode(reader, header).await.map_err(super::error)?;
            let Some(model) = observed.model.filter(|_| header.kind == "assistant" && !stats.synthetic) else {
                pending.stage = 2;
                return Ok(None);
            };
            let usage = match pending.usage.as_mut() {
                Some(usage) => usage,
                None => pending.usage.insert(Usage::new(reader, header).await?),
            };
            let (items, complete) = usage.advance(io, reader, header).await?;
            if complete {
                pending.stage = 2;
                pending.usage = None;
            }
            Ok(Some((
                "usage".into(),
                SessionFactV1::MessageUsage {
                    response: super::native(header.response.as_deref())?,
                    model: content::text(io, reader, Some(model)).await?,
                    stop_reason: content::optional_text(io, reader, observed.stop_reason).await?,
                    api_error: observed.api_error,
                    usage: match observed.usage {
                        Some(span) => Some(content::raw(io, reader, span).await?),
                        None => None,
                    },
                    items,
                },
                complete,
            )))
        }
        2 if header.compact_boundary || header.compact_summary => {
            let (preserved_ids, complete) = if let Some(compact) =
                fields::object(reader, root, "compactMetadata").await.map_err(super::error)?
                && let Some((at, _)) = reader.field(compact, "preservedMessages").await.map_err(super::error)?
                && let Some((uuids, _)) = reader.field(at, "uuids").await.map_err(super::error)?
            {
                let list = match pending.list.as_mut() {
                    Some(list) => list,
                    None => pending.list.insert(IdList::new(reader, uuids).await?),
                };
                let (reference, complete) = list.advance(io, reader).await?;
                (Some(reference), complete)
            } else {
                (None, true)
            };
            let preserved = context::preserved(reader, preserved_ids).await?;
            if complete {
                pending.stage = 3;
                pending.list = None;
            }
            Ok(Some((
                "compaction".into(),
                SessionFactV1::Compaction {
                    boundary: header.compact_boundary,
                    summary: header.compact_summary,
                    boundary_uuid: None,
                    replaces_context: header.kind == "system"
                        && fields::text(reader, root, "subtype").await.map_err(super::error)?.as_deref()
                            == Some("compact_boundary"),
                    preserved,
                    detail: content::raw(io, reader, reader.node(root).await.map_err(super::error)?.raw).await?,
                },
                complete,
            )))
        }
        2 => {
            pending.stage = 3;
            let hidden = stats.hidden;
            if !hidden && matches!(header.kind.as_str(), "assistant" | "user") {
                return Ok(None);
            }
            let host_kind =
                content::text(io, reader, message::string(reader, root, "type").await.map_err(super::error)?).await?;
            let detail = content::raw(io, reader, reader.node(root).await.map_err(super::error)?.raw).await?;
            Ok(Some((
                "event".into(),
                if hidden {
                    SessionFactV1::Hidden { host_kind, detail }
                } else {
                    SessionFactV1::Other { host_kind, detail }
                },
                true,
            )))
        }
        3 => {
            pending.stage = 4;
            let observed = message::decode(reader, header).await.map_err(super::error)?;
            let Some(reason) = observed.stop_reason.filter(|_| header.kind == "assistant") else { return Ok(None) };
            if reader.text(reason, 64).await.map_err(super::error)?.as_deref() != Some("end_turn") {
                return Ok(None);
            }
            Ok(Some((
                "exchange".into(),
                SessionFactV1::ExchangeEnded {
                    response: super::native(header.response.as_deref())?,
                    reason: content::text(io, reader, Some(reason)).await?,
                },
                true,
            )))
        }
        _ => {
            pending.stage = 4;
            Ok(None)
        }
    }
}

pub async fn has_items(
    io: &StepContent<'_>,
    reader: &Reader<'_>,
    header: &Header,
    row: &Row,
    rows: &mut Indexes<'_>,
) -> Result<bool, Error> {
    let stats = row_stats::get(io, reader, header, row, rows).await?;
    Ok(matches!(header.kind.as_str(), "user" | "assistant") && !header.compact_summary && !stats.hidden)
}
