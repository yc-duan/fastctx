//! Writing a history document empty deletes it: an agent takes a whole topic
//! out through the same controlled write as any change. Deleted, the topic is
//! retired, not erased: its versions stay, and the app can restore it.

use std::path::Path;

use fastctx_types::history::{HistoryCall, Scope};
use fastctx_types::write::{ChangeKind, FileChange, WriteOp, WriteOutcome, WriteResult};

/// What a writer is told who writes a document empty as it never read it.
pub(crate) const UNSEEN: &str = "writing it empty deletes this whole history document as it stands now; view it \
     with fastctx view first";

/// Whether `operation` writes its file empty, whitespace aside.
pub(crate) fn emptied(operation: &WriteOp) -> bool {
    matches!(operation, WriteOp::Write(_))
        && fastctx_hands::write::candidate::prepare(operation, None)
            .is_ok_and(|bytes| bytes.iter().all(u8::is_ascii_whitespace))
}

/// What deleting the document at `path` tells the writer; `previous` is
/// what it held.
pub(crate) fn told(path: &Path, previous: &str) -> WriteOutcome {
    let text = format!("write {}: deleted; the app can restore it", path.display());
    Ok(WriteResult {
        note_prefix: text.len(),
        text,
        changes: vec![FileChange {
            fact: None,
            path: path.to_path_buf(),
            kind: ChangeKind::Deleted,
            before_hash: Some(fastctx_hands::files::content_hash(previous.as_bytes())),
            after_hash: None,
            encoding: Some("utf-8".into()),
            encoding_selection: None,
        }],
        changed_since_seen: Vec::new(),
    })
}

/// The retirement of `name` in `scope` at its version `version`, or what
/// to tell a writer of a document that is not there.
pub(crate) fn request(scope: Scope, name: String, version: Option<u64>) -> Result<HistoryCall, WriteOutcome> {
    match version {
        Some(expected) => Ok(HistoryCall::Retire { scope, name, expected }),
        None => Err(Err("there is no such history document to delete".to_string().into())),
    }
}
