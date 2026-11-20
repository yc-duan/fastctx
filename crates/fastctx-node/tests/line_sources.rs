//! A reader holding a line source leaves the rest of it going: what waits on
//! the source for a change and what watches it read what it shows, never the
//! reader's lock, so a thread running them all goes on while a reader waits
//! for its lines to be taken (`fastctx_node::line_source_drill`).

use std::process::ExitCode;

fn main() -> ExitCode {
    match fastctx_node::line_source_drill() {
        Ok(()) => {
            println!("line sources: a reader holding one leaves the rest of it going");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("line sources: {error}");
            ExitCode::FAILURE
        }
    }
}
