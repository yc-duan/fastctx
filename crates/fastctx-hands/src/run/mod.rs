//! `run`: the World's shell (`world-run.md`).
//!
//! A call starts a job and waits fifteen seconds, or as long as the resident
//! asks, up to ten minutes. Whatever is not done by then goes on in the
//! background, under its own supervisor, and the resident who started it is
//! told when it ends. Every byte of output is kept in a log on the machine
//! where it ran.

mod cells;
pub mod control;
mod display;
mod graphics;
mod heredoc;
pub mod last;
pub mod launch;
mod linear;
#[cfg(target_os = "macos")]
mod login_shell;
mod output;
mod pieces;
mod plain;
pub mod preread;
pub mod present;
pub mod program;
pub mod projection;
mod queue;
pub mod record;
mod records;
mod remap;
pub mod shell;
pub mod sentinel;
pub mod walls;
pub mod supervisor;
pub mod terminal;
mod unfinished;
mod view_set;
pub mod window;
mod write_notes;

use std::time::Duration;

pub use control::stop;
pub use launch::{Launch, Launched, launch, launch_with_io};
pub use present::{Content, present};

/// How long a `run` call waits before its job goes on in the background,
/// unless the resident asks for another wait (`world-run.md`, ä¸€æ¬¡è°ƒç”¨ç­‰å¤šä¹…).
pub const WAIT: Duration = Duration::from_secs(15);

/// The longest wait a resident may ask for. Claude Code's own shell waits
/// at most this long in the foreground; the World's tool timeout for Codex
/// (`fastctx_hosts::codex::TOOL_TIMEOUT`) is set a minute above it.
pub const LONGEST_WAIT: Duration = Duration::from_secs(600);
pub use supervisor::supervise;
