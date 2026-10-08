//! `run`: the World's shell (`world-run.md`).
//!
//! A call starts a job and waits at most fifteen seconds. Whatever is not
//! done by then goes on in the background, under its own supervisor, and
//! the resident who started it is told when it ends. Every byte of output
//! is kept in a log on the machine where it ran.

pub mod last;
pub mod launch;
pub mod prelude;
pub mod present;
pub mod program;
pub mod record;
pub mod shell;
pub mod supervisor;
pub mod terminal;
pub mod window;

use std::time::Duration;

pub use launch::{Launch, Launched, launch};
pub use present::{Content, present};

/// How long a `run` call waits before its job goes on in the background
/// (`world-run.md`, 一次调用等 15 秒).
pub const WAIT: Duration = Duration::from_secs(15);
pub use supervisor::supervise;
