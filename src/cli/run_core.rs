//! Private JSON harness for the execution core, not a World or MCP tool.

use std::io;
use std::path::Path;

pub(super) fn execute() -> Result<(), String> {
    let request = serde_json::from_reader(io::stdin().lock()).map_err(|error| error.to_string())?;
    let result = crate::run::execute(request)?;
    serde_json::to_writer(io::stdout().lock(), &result).map_err(|error| error.to_string())
}

pub(super) fn reconcile(directory: &Path) -> Result<(), String> {
    let result = crate::run::reconcile(directory)?;
    serde_json::to_writer(io::stdout().lock(), &result).map_err(|error| error.to_string())
}
