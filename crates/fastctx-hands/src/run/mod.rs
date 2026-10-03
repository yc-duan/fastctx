//! `run`: the World's shell (`world-run.md`).
//!
//! A call starts a job and waits at most fifteen seconds. Whatever is not
//! done by then goes on in the background, under its own supervisor, and
//! the resident who started it is told when it ends. Every byte of output
//! is kept in a log on the machine where it ran.

pub mod launch;
pub mod record;
pub mod shell;
pub mod supervisor;
pub mod terminal;
pub mod window;

pub use launch::{Launch, Launched, launch};
pub use supervisor::supervise;
