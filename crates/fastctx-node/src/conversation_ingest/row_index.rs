//! Native-order traversal and once-only historical turn attribution.
use super::{
    claim::Claim,
    display_append,
    indexes::{self, Indexes},
    item_keys, parts,
    publisher::{block_key, identity, observed, original},
    render,
    row_state::*,
    step_content::StepContent,
};
use fastctx_hosts::claude::ingest::{
    document::{Address, Value, read::Reader},
    jsonl::Row,
    semantic::{self, children::Children, fields, row::Header},
};
use fastctx_sync::{
    cursor::{Count, Sequence},
    store::{Error, ingest_effects::SourceFamily},
};
use fastctx_types::sync::{
    conversation_identity::{NativeId, NativeTurnKey, TurnKey},
    conversation_timeline::*,
};

pub(super) async fn advance(
    claim: &Claim,
    io: &StepContent<'_>,
    reader: &Reader<'_>,
    header: &Header,
    row: &Row,
    local: Local<'_, '_>,
) -> Result<Option<TimelineFact>, Error> {
    let Local { rows, clock, work } = local;
    let uuid = header.uuid.as_deref().ok_or(Error::Busy)?;
    match work.stage {
        Stage::Index => {
            index(claim, reader, header, row, rows, work).await?;
            Ok(None)
        }
        Stage::Turn => turn(claim, header, Local::borrow(rows, clock, work)).await,
        Stage::Content => {
            if work.next >= work.total {
                work.stage = Stage::Finish;
                return Ok(None);
            }
            let key = block_key(claim, row, work.next.0);
            let address = rows
                .get::<Address>(SourceFamily::HostRows, &key)
                .await?
                .ok_or_else(|| Error::Corrupt("semantic block cursor is absent".into()))?;
            let value = reader.node(address).await.map_err(super::reading::error)?;
            let fact = if let Value::String { text } = value.value {
                if work.visible {
                    let part =
                        parts::text(io, &reader.document.bodies.strings, text, Count(0), DisplayChannel::Text, 4096)
                            .await
                            .map_err(super::reading::error)?;
                    let mut display = render::empty(DisplayKind::User, None, DisplayStatus::Complete);
                    display_append::append(io, &mut display, part).await?;
                    Some(
                        observed(
                            io,
                            row,
                            Local::borrow(rows, clock, work),
                            item_keys::user(&claim.conversation, uuid).map_err(identity)?,
                            display,
                            true,
                        )
                        .await?,
                    )
                } else {
                    None
                }
            } else {
                let block = semantic::block::decode(reader, address).await.map_err(super::reading::error)?;
                super::row_content::content(claim, io, reader, header, row, Local::borrow(rows, clock, work), &block)
                    .await?
            };
            if work.nested.is_some() {
                return Ok(fact);
            }
            rows.delete(SourceFamily::HostRows, &key).await?;
            work.next.0 += 1;
            if work.next >= work.total {
                work.stage = Stage::Finish;
                rows.put(
                    SourceFamily::HostRows,
                    &indexes::key("row", uuid),
                    &NativeRow {
                        body: original(row).id().digest.clone(),
                        turn: work.placement.as_ref().map(|p| p.turn.clone()),
                    },
                )
                .await?;
            }
            Ok(fact)
        }
        Stage::Finish => Ok(None),
    }
}
async fn index(
    claim: &Claim,
    reader: &Reader<'_>,
    header: &Header,
    row: &Row,
    rows: &mut Indexes<'_>,
    work: &mut Work,
) -> Result<(), Error> {
    if let Some((parent, children)) = &mut work.index_nested {
        for _ in 0..48 {
            let Some((address, node)) = children.step(reader).await.map_err(super::reading::error)? else { break };
            rows.put(
                SourceFamily::HostRows,
                &super::row_content::nested_key(claim, row, parent.0, node.ordinal.0),
                &address,
            )
            .await?;
        }
        if children.remaining.0 == 0 {
            work.index_nested = None;
        }
        return Ok(());
    }
    if work.children.is_none()
        && work.total.0 == 0
        && let Some(content) = header.content
    {
        work.children = Children::of(reader, content).await.map_err(super::reading::error)?;
        work.total = work.children.as_ref().map_or(Count(1), |c| c.remaining);
        if work.children.is_none() {
            rows.put(SourceFamily::HostRows, &block_key(claim, row, 0), &content).await?;
            work.visible = header.prompt_candidate;
            if let Value::String { text } = reader.node(content).await.map_err(super::reading::error)?.value {
                work.first = Some(text);
            }
        }
    }
    if let Some(children) = &mut work.children {
        for _ in 0..48 {
            let Some((address, node)) = children.step(reader).await.map_err(super::reading::error)? else { break };
            rows.put(SourceFamily::HostRows, &block_key(claim, row, node.ordinal.0), &address).await?;
            let kind = fields::text(reader, address, "type").await.map_err(super::reading::error)?;
            work.has_result |= kind.as_deref() == Some("tool_result");
            if kind.as_deref() == Some("tool_result")
                && let Some((content, _)) = reader.field(address, "content").await.map_err(super::reading::error)?
                && let Some(nested) = Children::of(reader, content).await.map_err(super::reading::error)?
                && nested.remaining.0 > 0
            {
                work.index_nested = Some((node.ordinal, nested));
                return Ok(());
            }
            work.visible |= matches!(kind.as_deref(), Some("text" | "image"));
            if let Some((_, n)) = reader.field(address, "text").await.map_err(super::reading::error)?
                && let Value::String { text } = n.value
                && !fields::leading(reader, text, 64).await.map_err(super::reading::error)?.is_empty()
            {
                work.first = Some(text);
            }
        }
        if children.remaining.0 > 0 {
            return Ok(());
        }
    }
    let excluded = if let Some(span) = work.first {
        semantic::prompt::excluded_prompt(&fields::leading(reader, span, 64).await.map_err(super::reading::error)?)
    } else {
        false
    };
    work.visible &= header.prompt_candidate && !work.has_result && !excluded;
    work.stage = Stage::Turn;
    Ok(())
}
async fn turn(claim: &Claim, header: &Header, local: Local<'_, '_>) -> Result<Option<TimelineFact>, Error> {
    let Local { rows, clock, work } = local;
    let uuid = header.uuid.as_deref().ok_or(Error::Busy)?;
    if work.visible {
        let old = rows.get::<NativeRow>(SourceFamily::HostRows, &indexes::key("row", uuid)).await?;
        let turn = TurnKey {
            conversation: claim.conversation.clone(),
            native: NativeTurnKey::Claude {
                session: claim.session.clone(),
                turn: NativeId::new(uuid).map_err(identity)?,
            },
        };
        let chosen = old.and_then(|old| old.turn).unwrap_or_else(|| turn.id());
        work.placement = Some(Placement { turn: chosen.clone(), slot: DisplaySlot::Opening, proof: bootstrap()? });
        rows.put(
            SourceFamily::HostRows,
            &format!("claude/session/{}", claim.primary.source.as_str()),
            &Session { turn: chosen.clone() },
        )
        .await?;
        work.stage = Stage::Content;
        if rows.get::<Sequence>(SourceFamily::HostRows, &format!("claude/turn/{}", chosen.as_str())).await?.is_none() {
            if chosen != turn.id() {
                return Err(Error::Corrupt("row turn has no retained declaration".into()));
            }
            let order = clock.turn()?;
            rows.put(SourceFamily::HostRows, &format!("claude/turn/{}", chosen.as_str()), &order).await?;
            return Ok(Some(TimelineFact::Turn { turn: TurnDeclaration { key: turn, order } }));
        }
    } else {
        let parent = match header.parent.as_deref() {
            Some(parent) => {
                rows.get::<NativeRow>(SourceFamily::HostRows, &indexes::key("row", parent)).await?.and_then(|r| r.turn)
            }
            None => None,
        };
        let session = rows
            .get::<Session>(SourceFamily::HostRows, &format!("claude/session/{}", claim.primary.source.as_str()))
            .await?;
        work.placement = parent
            .or_else(|| session.map(|s| s.turn))
            .map(|turn| Ok::<_, Error>(Placement { turn, slot: DisplaySlot::Sibling, proof: bootstrap()? }))
            .transpose()?;
        work.stage = Stage::Content;
    }
    Ok(None)
}
fn bootstrap() -> Result<PlacementProof, Error> {
    Ok(PlacementProof::Bootstrap { method: NativeId::new("claude-prompt-chain-v1").map_err(identity)? })
}
