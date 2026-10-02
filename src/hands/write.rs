//! The World's write tools on this machine: `edit`, `write` and `apply_patch` over the shared
//! write core (`specs/world-write.md`), tied to the resident that calls them.
//!
//! What the resident has seen of a file comes from its `seen` record on this machine
//! (`town::seen`): a whole-file view gives the content hash the core compares against, a partial
//! view gives size and modification time, which `write` checks before replacing a whole file.
//! The town's ledger (charters and journals) is never written through these tools; journals
//! change only through `fastctx journal`.

use crate::town::core::Town;
use crate::town::seen::{self, Seen};
use crate::write::{ApplyPatchRequest, EditRequest, WriteContext, WriteError, WriteRequest, WriteResult};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Who is writing: the resident, and where its records are on this machine.
pub(crate) struct Writer {
    town: Arc<Town>,
    resident: String,
    records: Option<PathBuf>,
}

impl Writer {
    pub(crate) fn new(town: Arc<Town>, resident: String) -> Self {
        let records = town.town_of(&resident).map(|place| town.resident_dir(place.as_deref(), &resident));
        Self { town, resident, records }
    }

    fn seen(&self, path: &Path) -> Option<Seen> {
        seen::latest(self.records.as_deref()?, path)
    }

    fn record(&self, hand: &str, what: &str, outcome: &Result<WriteResult, WriteError>) {
        let result = match outcome {
            Ok(result) => format!("{} changed", result.changes.len()),
            Err(error) => format!("refused: {}", error.message.lines().next().unwrap_or_default()),
        };
        self.town.record_action(&self.resident, self.town.machine(), hand, what, &result);
    }

    /// Whether a path lies in a town's ledger, which only the World writes.
    fn ledger(&self, path: &Path) -> bool {
        let towns = self.town.world_dir().join("towns");
        let Ok(rest) = path.strip_prefix(&towns) else { return false };
        rest.components().nth(1).is_some_and(|component| component.as_os_str() == "ledger")
    }
}

pub(crate) fn edit(writer: &Writer, request: EditRequest) -> Result<WriteResult, WriteError> {
    let path = PathBuf::from(&request.file_path);
    let seen = writer.seen(&path);
    let what = request.file_path.clone();
    let outcome = run(writer, seen.as_ref(), |context| crate::write::edit(request, context));
    writer.record("edit", &what, &outcome);
    outcome
}

pub(crate) fn write(writer: &Writer, request: WriteRequest) -> Result<WriteResult, WriteError> {
    let path = PathBuf::from(&request.file_path);
    let seen = writer.seen(&path);
    let what = request.file_path.clone();
    // A file seen only in part is checked by its size and modification time; one seen whole is
    // checked by the core against its hash.
    if let Some(seen) = &seen
        && seen.hash.is_none()
        && seen.changed_since().map_err(WriteError::from)?
    {
        let outcome = Err(WriteError::from(format!(
            "{} changed after you viewed it; view it again before replacing the whole file.",
            request.file_path
        )));
        writer.record("write", &what, &outcome);
        return outcome;
    }
    let outcome = run(writer, seen.as_ref(), |context| crate::write::write(request, context));
    writer.record("write", &what, &outcome);
    outcome
}

pub(crate) fn apply_patch(writer: &Writer, request: ApplyPatchRequest) -> Result<WriteResult, WriteError> {
    let what = patch_files(&request.input);
    let outcome = run(writer, None, |context| crate::write::apply_patch(request, context));
    writer.record("apply_patch", &what, &outcome);
    outcome
}

fn run(
    writer: &Writer,
    seen: Option<&Seen>,
    operation: impl FnOnce(&WriteContext<'_>) -> Result<WriteResult, WriteError>,
) -> Result<WriteResult, WriteError> {
    let encodings = |path: &Path| writer.seen(path).and_then(|seen| seen.encoding);
    let ledger = |path: &Path| writer.ledger(path);
    let context = WriteContext {
        node: writer.town.machine(),
        observed_hash: seen.and_then(|seen| seen.hash.as_deref()),
        source_encoding: Some(&encodings),
        denied_directory: Some(&ledger),
    };
    operation(&context)
}

/// The files a patch names, for the action line.
fn patch_files(input: &str) -> String {
    let files: Vec<&str> = input
        .lines()
        .filter_map(|line| {
            ["*** Add File: ", "*** Update File: ", "*** Delete File: "].iter().find_map(|head| line.strip_prefix(head)).map(str::trim)
        })
        .collect();
    files.join(", ")
}
