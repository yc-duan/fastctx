//! Dual-mode MCP, CLI, and TUI process entry point for fastctx.

use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    match fastctx::cli::run().await {
        Ok(code) => code,
        Err(error) => {
            eprintln!("fastctx: {error}");
            ExitCode::FAILURE
        }
    }
}
