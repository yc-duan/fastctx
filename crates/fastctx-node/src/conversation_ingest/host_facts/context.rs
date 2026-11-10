//! Native context evidence is public structure, independent of host JSON detail.
use super::content;
use crate::conversation_ingest::step_content::StepContent;
use fastctx_hosts::claude::ingest::{
    document::read::Reader,
    semantic::{fields, message::string, row::Header},
};
use fastctx_sync::store::Error;
use fastctx_types::sync::{conversation_timeline::*, manifest::ContentRef};

pub async fn native_row(
    io: &StepContent<'_>,
    reader: &Reader<'_>,
    header: &Header,
    supersedes: Option<ContentRef>,
) -> Result<ContentRef, Error> {
    let root = reader.document.root;
    let flag =
        async |name| fields::boolean(reader, root, name).await.map(|flag| flag.unwrap_or(false)).map_err(super::error);
    content::publish(
        io,
        ItemFactV1::NativeRow {
            uuid: super::native(header.uuid.as_deref())?,
            parent: super::native(header.parent.as_deref())?,
            logical_parent: super::native(
                fields::text(reader, root, "logicalParentUuid").await.map_err(super::error)?.as_deref(),
            )?,
            source_assistant: super::native(header.source_assistant.as_deref())?,
            host_kind: content::text(io, reader, string(reader, root, "type").await.map_err(super::error)?).await?,
            origin: match fields::object(reader, root, "origin").await.map_err(super::error)? {
                Some(origin) => {
                    content::optional_text(io, reader, string(reader, origin, "kind").await.map_err(super::error)?)
                        .await?
                }
                None => None,
            },
            flags: NativeRowFlagsV1 {
                meta: flag("isMeta").await?,
                virtual_message: flag("isVirtual").await?,
                compact_summary: header.compact_summary,
                transcript_only: flag("isVisibleInTranscriptOnly").await?,
                turn_companion: flag("turnCompanion").await?,
                dispatch_echo: flag("dispatchEcho").await?,
                stacked_expansion: flag("stackedExpansion").await?,
            },
            supersedes,
        },
    )
    .await
}
/// Preserved UUID lists are supplied separately by the bounded event builder.
pub async fn preserved(reader: &Reader<'_>, uuids: Option<ContentRef>) -> Result<Option<PreservedContextV1>, Error> {
    let Some(compact) = fields::object(reader, reader.document.root, "compactMetadata").await.map_err(super::error)?
    else {
        return Ok(None);
    };
    let read_id = async |at, name| {
        let value = fields::text(reader, at, name)
            .await
            .map_err(super::error)?
            .ok_or_else(|| super::invalid("preserved context omits a native UUID"))?;
        fastctx_types::sync::conversation_identity::NativeId::new(value).map_err(super::invalid)
    };
    if let Some((at, _)) = reader.field(compact, "preservedMessages").await.map_err(super::error)? {
        let anchor = read_id(at, "anchorUuid").await?;
        let uuids = uuids.ok_or_else(|| super::invalid("preserved messages omit a UUID list"))?;
        return Ok(Some(PreservedContextV1 { anchor, selection: PreservedSelectionV1::Messages { uuids } }));
    }
    if let Some(at) = fields::object(reader, compact, "preservedSegment").await.map_err(super::error)? {
        return Ok(Some(PreservedContextV1 {
            anchor: read_id(at, "anchorUuid").await?,
            selection: PreservedSelectionV1::Segment {
                head: read_id(at, "headUuid").await?,
                tail: read_id(at, "tailUuid").await?,
            },
        }));
    }
    Ok(None)
}

#[derive(serde::Serialize, serde::Deserialize)]
struct ContextLink {
    parent: Option<fastctx_types::sync::conversation_identity::NativeId>,
    boundary: bool,
    message: bool,
}
pub async fn remember(
    rows: &mut crate::conversation_ingest::indexes::Indexes<'_>,
    header: &Header,
) -> Result<(), Error> {
    let Some(uuid) = &header.uuid else { return Ok(()) };
    rows.put(
        fastctx_sync::store::ingest_effects::SourceFamily::HostRows,
        &crate::conversation_ingest::indexes::key("context-link", uuid),
        &ContextLink {
            parent: super::native(header.parent.as_deref())?,
            boundary: header.compact_boundary,
            message: matches!(header.kind.as_str(), "user" | "assistant"),
        },
    )
    .await
}
pub async fn link_compaction(
    rows: &mut crate::conversation_ingest::indexes::Indexes<'_>,
    header: &Header,
    event: &mut SessionFactV1,
) -> Result<(), Error> {
    use crate::conversation_ingest::indexes::key;
    use fastctx_sync::store::ingest_effects::SourceFamily;
    let SessionFactV1::Compaction { boundary, boundary_uuid, summary, preserved, .. } = event else { return Ok(()) };
    if *boundary {
        *boundary_uuid = super::native(header.uuid.as_deref())?;
        if let (Some(boundary), Some(preserved)) = (boundary_uuid, preserved) {
            rows.put(SourceFamily::HostRows, &key("compact-anchor", preserved.anchor.as_str()), boundary).await?;
        }
    } else if *summary {
        if let Some(uuid) = &header.uuid {
            *boundary_uuid = rows.get(SourceFamily::HostRows, &key("compact-anchor", uuid)).await?;
            if boundary_uuid.is_some() {
                return Ok(());
            }
        }
        let mut parent = super::native(header.parent.as_deref())?;
        for _ in 0..32 {
            let Some(uuid) = parent else { break };
            let Some(link) =
                rows.get::<ContextLink>(SourceFamily::HostRows, &key("context-link", uuid.as_str())).await?
            else {
                break;
            };
            if link.boundary {
                *boundary_uuid = Some(uuid);
                break;
            }
            if link.message {
                break;
            }
            parent = link.parent;
        }
    }
    Ok(())
}
