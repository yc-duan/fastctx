//! The public patch grammar is normalized during ingestion, one file at a time.
use super::{
    content::{self, TextWriter},
    forward_list::ForwardList,
};
use crate::conversation_ingest::step_content::StepContent;
use fastctx_hosts::claude::ingest::{
    document::{Span, read::Reader},
    semantic::{
        fields,
        lines::{self, Lines},
    },
};
use fastctx_sync::store::Error;
use fastctx_types::sync::{conversation_timeline::*, cursor::ByteCount, manifest::ContentRef};

struct Change<'a, 'source> {
    path: ContentRef,
    how: FactChangeV1,
    diff: TextWriter<'a, 'source>,
}
pub async fn changes(io: &StepContent<'_>, reader: &Reader<'_>, span: Option<Span>) -> Result<ContentRef, Error> {
    let mut list = ForwardList::new(io, FactPartKindV1::Change);
    let mut current: Option<Change<'_, '_>> = None;
    if let Some(span) = span {
        let mut lines = Lines::new(span);
        while let Some(line) = lines.next(reader).await.map_err(super::error)? {
            let prefix = fields::prefix(reader, line, 32).await.map_err(super::error)?;
            let header = [
                ("*** Add File: ", FactChangeV1::Added),
                ("*** Delete File: ", FactChangeV1::Deleted),
                ("*** Update File: ", FactChangeV1::Updated),
            ]
            .into_iter()
            .find(|(head, _)| prefix.starts_with(head));
            if let Some((head, how)) = header {
                if let Some(change) = current.take() {
                    list.push(finish(change).await?).await?;
                }
                let path = after(line, head.len());
                let path = lines::trim(reader, path).await.map_err(super::error)?;
                current =
                    Some(Change { path: content::text(io, reader, Some(path)).await?, how, diff: TextWriter::new(io) });
                continue;
            }
            let Some(change) = &mut current else { continue };
            if prefix.starts_with("*** Move to: ") {
                let to = lines::trim(reader, after(line, "*** Move to: ".len())).await.map_err(super::error)?;
                change.how = FactChangeV1::Moved { to: content::text(io, reader, Some(to)).await? };
            } else if !prefix.starts_with("*** End")
                && (prefix.starts_with(['+', '-', ' ']) || prefix.starts_with("@@") || line.bytes.0 == 0)
            {
                change.diff.span(reader, line).await?;
                change.diff.push(b"\n").await?;
            }
        }
    }
    if let Some(change) = current {
        list.push(finish(change).await?).await?;
    }
    list.finish().await
}
async fn finish(change: Change<'_, '_>) -> Result<FactPartV1, Error> {
    Ok(FactPartV1::Change {
        path: change.path,
        how: change.how,
        diff: FactTextV1::Content { content: change.diff.finish().await? },
    })
}
fn after(span: Span, bytes: usize) -> Span {
    Span { offset: ByteCount(span.offset.0 + bytes as u64), bytes: ByteCount(span.bytes.0 - bytes as u64) }
}
