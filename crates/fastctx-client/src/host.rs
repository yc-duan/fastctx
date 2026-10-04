//! Running an agent's own host program in the user's terminal, as
//! `fastctx attach` and the TUI do: it has the terminal to itself until it
//! exits.

use std::io;
use std::process::{Command, ExitStatus};

use fastctx_types::calls::HostCommand;

/// Runs `command` with this terminal as its own, and waits for it.
pub fn run(command: &HostCommand) -> io::Result<ExitStatus> {
    Command::new(&command.program)
        .args(&command.args)
        .envs(command.env.iter().map(|(name, value)| (name, value)))
        .status()
        .map_err(|error| io::Error::new(error.kind(), format!("starting {}: {error}", command.program.display())))
}
