//! Foreground command supervision with timeout and output-independent process life.

use crate::model::ToolResponse;
use crate::shell::encoding::OutputEncoding;
use crate::shell::output::{capture_foreground, format_foreground};
use crate::shell::process::{exit_code, spawn_bash};
use std::path::Path;
use std::time::{Duration, Instant};

/// Runs one command while reserving enough time for MCP response serialization.
pub(crate) fn run(
    bash: &Path,
    command: &str,
    cwd: &Path,
    timeout_ms: u64,
    login_shell: bool,
    encoding: Option<OutputEncoding>,
    cancelled: impl Fn() -> bool,
) -> ToolResponse {
    let mut process = match spawn_bash(bash, command, cwd, login_shell) {
        Ok(process) => process,
        Err(error) => return ToolResponse::error(format!("Cannot start the command: {error}.")),
    };
    let reader = process.take_output();
    let reader_thread = std::thread::spawn(move || capture_foreground(reader));

    let started = Instant::now();
    let mut timed_out = false;
    let status = loop {
        match process.try_wait() {
            Ok(Some(status)) => {
                // A shell that detached descendants must not leave them outside this tool's lifetime.
                if let Err(error) = process.terminate_tree() {
                    drop(reader_thread);
                    return ToolResponse::error(format!(
                        "The command exited, but its remaining process tree could not be terminated: {error}. Stop the descendants manually before retrying."
                    ));
                }
                break status;
            }
            Ok(None) => {}
            Err(error) => {
                if let Err(kill_error) = process.terminate_tree() {
                    drop(reader_thread);
                    return ToolResponse::error(format!(
                        "Cannot monitor the command process ({error}), and its process tree could not be terminated ({kill_error}). Stop it manually and retry."
                    ));
                }
                let _ = reader_thread.join();
                return ToolResponse::error(format!(
                    "Cannot monitor the command process: {error}. Retry the command."
                ));
            }
        }
        if cancelled() {
            if let Err(error) = process.terminate_tree() {
                drop(reader_thread);
                return ToolResponse::error(format!(
                    "The command was cancelled, but its process tree could not be terminated: {error}. Stop it manually before retrying."
                ));
            }
            let _ = reader_thread.join();
            return ToolResponse::error(
                "The command was cancelled and its process tree was terminated.".to_string(),
            );
        }
        if started.elapsed() >= Duration::from_millis(timeout_ms) {
            timed_out = true;
            match process.terminate_tree() {
                Ok(status) => break status,
                Err(error) => {
                    drop(reader_thread);
                    return ToolResponse::error(format!(
                        "The command timed out after {timeout_ms} ms, but its process tree could not be terminated: {error}. Stop it manually and retry."
                    ));
                }
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    };

    let captured = match reader_thread.join() {
        Ok(Ok(output)) => output,
        Ok(Err(error)) => {
            return ToolResponse::error(format!(
                "Cannot read command output: {error}. Retry the command or redirect output to a file."
            ));
        }
        Err(_) => {
            return ToolResponse::error(
                "Internal tool failure: the command-output reader panicked.".to_string(),
            );
        }
    };
    format_foreground(
        &captured,
        exit_code(status),
        timed_out.then_some(timeout_ms),
        encoding,
    )
}
