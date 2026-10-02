//! Stdin adapter for the shared synchronous patch core.

use crate::write::{ApplyPatchRequest, WriteContext};
use std::io::{self, Read, Write};
use std::process::ExitCode;

pub(super) fn run() -> Result<ExitCode, String> {
    let mut input = String::new();
    io::stdin()
        .read_to_string(&mut input)
        .map_err(|error| format!("Cannot read patch from stdin: {error}"))?;
    if input.is_empty() {
        eprintln!("Usage: fastctx apply_patch < patch-file");
        return Ok(ExitCode::from(2));
    }
    let result = crate::write::apply_patch(ApplyPatchRequest { input }, &WriteContext::default())
        .map_err(|error| error.to_string())?;
    writeln!(io::stdout().lock(), "{}", result.text)
        .map_err(|error| format!("Cannot write patch result: {error}"))?;
    Ok(ExitCode::SUCCESS)
}
