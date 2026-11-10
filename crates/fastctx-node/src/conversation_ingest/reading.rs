//! Advance one persisted decoder window. A complete row becomes private Publishing
//! state before any public semantic observation is constructed.
use super::{bodies::Bodies, step_content::StepContent};
use fastctx_hosts::claude::ingest::{
    body::{BodyKey, BodyPart},
    checkpoint::{AuditCursor, ClaudeCheckpointV1, PARSER_VERSION, Partial, Phase, ReadAhead},
    file::Window,
    jsonl::Jsonl,
};
use fastctx_sync::{
    cursor::ByteCount,
    manifest::{ContentBlock, ContentRef},
    store::Error,
};
use fastctx_types::file_identity::{FileChangeStamp, FileIdentity};

pub struct Advance {
    pub checkpoint: ClaudeCheckpointV1,
    pub committed: ByteCount,
}

/// The caller obtains `window` through the observed native read port, unless an
/// earlier checkpoint already owns unread bytes. Only parsed bytes reach JSONL.
pub async fn advance(
    content: &StepContent<'_>,
    previous: Option<&ClaudeCheckpointV1>,
    window: Option<Window>,
) -> Result<Advance, Error> {
    let step = content.target().step();
    let mut checkpoint = match previous {
        Some(old) => old.clone(),
        None => {
            let sample = window.as_ref().ok_or_else(|| Error::Invalid("initial source window is absent".into()))?;
            initial(sample.before.identity(), sample.before.change_stamp().map_err(error)?)
        }
    };
    if !matches!(checkpoint.phase, Phase::Following | Phase::SourceMissing) {
        return Err(Error::Invalid("decoder cannot advance an unreconciled source".into()));
    }
    let mut decoder = match checkpoint.partial.take() {
        Some(Partial::Reading { line }) => Jsonl::resume(line).map_err(error)?,
        Some(Partial::Publishing { .. }) => return Err(Error::Invalid("validated row still needs publication".into())),
        None => Jsonl::new(
            BodyKey {
                source: step.primary().source.clone(),
                generation: step.generation().clone(),
                line_start: checkpoint.read_offset,
                part: BodyPart::Raw,
            },
            ByteCount(0),
        ),
    };
    let (bytes, start, consumed, stored) = match checkpoint.read_ahead.take() {
        Some(ahead) => {
            if window.is_some() {
                return Err(Error::Invalid("native read supplied before staged tail was consumed".into()));
            }
            let ContentRef::Single { block, decoded_bytes } = &ahead.content else {
                return Err(Error::Corrupt("read-ahead is not a bounded block".into()));
            };
            let bytes = content.state_block(ContentBlock { id: block.clone(), decoded_bytes: *decoded_bytes }).await?;
            (bytes, ahead.start, ahead.consumed as usize, Some(ahead.content))
        }
        None => {
            let window = window.ok_or_else(|| Error::Invalid("source bytes are absent".into()))?;
            if window.offset != checkpoint.read_offset || window.after.identity() != checkpoint.file_identity {
                return Err(Error::Invalid("native window differs from the parser cursor".into()));
            }
            checkpoint.observed_stamp = window.after.change_stamp().map_err(error)?;
            (window.bytes, window.offset, 0, None)
        }
    };
    if consumed >= bytes.len() {
        return Err(Error::Invalid("decoder advance needs unread source bytes".into()));
    }
    let mut sink = Bodies { content };
    let progress = decoder.push(&bytes[consumed..], &mut sink).await.map_err(error)?;
    let used = consumed.checked_add(progress.used).ok_or(Error::Busy)?;
    checkpoint.read_offset = decoder.read_offset();
    checkpoint.phase = Phase::Following;
    if used < bytes.len() {
        let reference = match stored {
            Some(reference) => reference,
            None => content.put_block(bytes.clone()).await?.into(),
        };
        checkpoint.read_ahead =
            Some(ReadAhead { start, bytes: bytes.len() as u32, consumed: used as u32, content: reference });
    }
    let committed = if let Some(row) = progress.row {
        let committed = row.start;
        checkpoint.partial = Some(Partial::Publishing {
            source: step.primary().source.clone(),
            generation: step.generation().clone(),
            row,
            consumer: None,
        });
        committed
    } else {
        let committed = decoder.committed_offset();
        checkpoint.partial = Some(Partial::Reading { line: decoder.checkpoint(&mut sink).await.map_err(error)? });
        committed
    };
    Ok(Advance { checkpoint, committed })
}

fn initial(file_identity: FileIdentity, observed_stamp: FileChangeStamp) -> ClaudeCheckpointV1 {
    ClaudeCheckpointV1 {
        version: PARSER_VERSION,
        file_identity,
        observed_stamp: observed_stamp.clone(),
        read_offset: ByteCount(0),
        partial: None,
        read_ahead: None,
        anchors_root: None,
        row_index_root: None,
        phase: Phase::Following,
        audit_cursor: AuditCursor { observation: observed_stamp, through: ByteCount(0) },
    }
}
pub(super) fn error(error: std::io::Error) -> Error {
    if let Some(superseded) = super::state_read::Superseded::from_io(&error) {
        return Error::LocalConflict { current: superseded.current.unwrap_or(fastctx_sync::cursor::Revision(0)) };
    }
    if error.get_ref().is_some_and(|inner| inner.is::<Error>()) {
        return match error.into_inner().and_then(|inner| inner.downcast::<Error>().ok()) {
            Some(error) => *error,
            None => Error::Corrupt("decoder lost its typed source error".into()),
        };
    }
    Error::Invalid(error.to_string())
}
