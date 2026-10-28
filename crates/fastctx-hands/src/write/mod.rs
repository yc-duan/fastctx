//! The local synchronous core behind `edit`, `write`, and `apply_patch`.
//!
//! All writers in one World share a process lock. Each file is published whole;
//! a multi-file patch is sequential, not a transaction, matching Codex. Failures
//! retain the list of changes already committed. The node should call this core
//! on a blocking thread and persist the returned changes, including on failure.
//!
//! The lock coordinates World writers only. The current bytes are rechecked after
//! staging and before rename, but that is not a kernel compare-and-swap against
//! programs that write outside the World; such a program can race the final rename.

mod blocks;
pub mod candidate;
mod document;
mod edit;
mod excerpts;
mod facts;
mod patch;
mod path;
mod storage;

pub use path::absolute_path;

use crate::files::canonical_path;
use fastctx_platform::lock::FileLock;
use fastctx_types::files::SeenFiles;
use fastctx_types::layout::WorldDir;
pub use fastctx_types::write::*;
use std::io;
use std::path::PathBuf;

/// Captures an already committed ledger write in the same format as ordinary file writes.
pub fn capture_change(
    directory: PathBuf,
    machine: fastctx_types::MachineName,
    path: &std::path::Path,
    before: Option<&[u8]>,
    after: &[u8],
) -> Result<fastctx_types::run_facts::WriteFact, String> {
    facts::Capture::new(directory, machine)
        .map_err(|error| error.to_string())?
        .prepare(path, before, Some(after), None, None, 0)
        .map(|fact| *fact)
}

/// Applies the same protected-path policy to non-editor file operations.
pub fn check_path_policy(world: &std::path::Path, path: &std::path::Path) -> Result<(), String> {
    let writer =
        Writer { world: canonical_path(world).map_err(|error| error.to_string())?, node: String::new(), capture: None };
    storage::Target::new(path, &writer).map(|_| ())
}

/// All state is explicit: this core never reads the user's default fastctx home.
pub struct Writer {
    world: PathBuf,
    node: String,
    capture: Option<facts::Capture>,
}

impl Writer {
    pub fn new(world: &WorldDir, node: impl Into<String>) -> io::Result<Self> {
        if !world.path().is_absolute() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "World directory must be absolute"));
        }
        std::fs::create_dir_all(world.path())?;
        Ok(Self { world: canonical_path(world.path())?, node: node.into(), capture: None })
    }

    pub fn with_facts(mut self, directory: PathBuf, machine: fastctx_types::MachineName) -> io::Result<Self> {
        self.capture = Some(facts::Capture::new(directory, machine)?);
        Ok(self)
    }

    pub fn edit(&self, request: EditRequest, seen: &SeenFiles) -> Result<WriteResult, WriteError> {
        let _lock = self.lock().map_err(|error| self.failure("edit", error))?;
        edit::edit(self, request, seen)
            .map(|result| self.noted(result, "edit"))
            .map_err(|error| self.failure("edit", error))
    }

    pub fn edit_blocks(&self, request: EditBlocksRequest, seen: &SeenFiles) -> Result<WriteResult, WriteError> {
        let _lock = self.lock().map_err(|error| self.failure("edit", error))?;
        edit::edit_blocks(self, request, seen)
            .map(|result| self.noted(result, "edit"))
            .map_err(|error| self.failure("edit", error))
    }

    pub fn write(&self, request: WriteRequest, seen: &SeenFiles) -> Result<WriteResult, WriteError> {
        let _lock = self.lock().map_err(|error| self.failure("write", error))?;
        edit::write(self, request, seen)
            .map(|result| self.noted(result, "write"))
            .map_err(|error| self.failure("write", error))
    }

    pub fn apply_patch(&self, request: ApplyPatchRequest, seen: &SeenFiles) -> Result<WriteResult, WriteError> {
        let _lock = self.lock().map_err(|error| self.failure("apply_patch", error))?;
        patch::apply_patch(self, request, seen)
            .map(|result| self.noted(result, "apply_patch"))
            .map_err(|error| self.failure("apply_patch", error))
    }

    fn noted(&self, mut result: WriteResult, operation: &str) -> WriteResult {
        use fastctx_types::run_facts::{NoteCommand, WriteNote};
        let command = match operation {
            "edit" => NoteCommand::Edit,
            "write" => NoteCommand::Write,
            "apply_patch" => NoteCommand::ApplyPatch,
            _ => unreachable!(),
        };
        // CLI success is writeln!(stdout, result.text). Edit excerpts follow the note.
        let text = if result.note_prefix == result.text.len() {
            format!("{}\n", result.text)
        } else {
            result.text[..result.note_prefix].to_string()
        };
        let first = result.changes.iter().find_map(|change| change.fact.as_ref().map(|fact| fact.id.clone()));
        for change in &mut result.changes {
            if let Some(fact) = &mut change.fact {
                fact.note = Some(if first.as_ref() == Some(&fact.id) {
                    WriteNote::Text { command, text: text.clone() }
                } else {
                    WriteNote::Same { fact: first.clone().expect("the operation's first fact") }
                });
            }
        }
        result
    }

    fn lock(&self) -> Result<FileLock, WriteError> {
        FileLock::acquire(&self.world.join("write.lock")).map_err(|e| format!("Cannot lock world writes: {e}").into())
    }

    fn head(&self, operation: &str, changes: usize) -> String {
        format!("=== {operation} on {} ({}) ===", self.node, counted(changes))
    }

    /// The head note of one file written: the operation, the file as `view`
    /// names it, the machine, and how it went.
    fn file_head(&self, operation: &str, path: &std::path::Path, outcome: &str) -> String {
        let path = path.to_string_lossy();
        let path = if cfg!(windows) { path.replace('\\', "/") } else { path.into_owned() };
        format!("=== {operation} {path} on {} ({outcome}) ===", self.node)
    }

    fn failure(&self, operation: &str, mut error: WriteError) -> WriteError {
        let state = match error.changes.len() {
            0 => "failed; nothing changed".to_string(),
            n => format!("failed after {}", counted(n)),
        };
        let note = format!("=== {operation} on {} ({state}) ===\n", self.node);
        error.note_prefix = note.len();
        error.message = format!("{note}{}", error.message);
        error
    }
}

fn counted(changes: usize) -> String {
    match changes {
        1 => "1 change".to_string(),
        n => format!("{n} changes"),
    }
}
