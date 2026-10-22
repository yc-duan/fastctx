//! Putting the World's tools into the sessions the user opens on this
//! machine, and taking them out again (`world-run.md`, 模型怎样知道;
//! `world-upgrade.md` for what an old version left behind).
//!
//! A World-started session gets the World's tools when it is started. The
//! user's own sessions read them from their host's configuration, and this
//! crate writes it:
//! - Codex: the World's server in `config.toml`, with the limits that keep
//!   Codex from cutting an answer the World has already sized, and a block
//!   of guidance in `AGENTS.md`;
//! - Claude Code: the World's plugin, from the marketplace the node keeps,
//!   installed with Claude Code's own `claude plugin` commands; the output
//!   limit in `settings.json`; a block of guidance in `CLAUDE.md`.
//!
//! What is written is recorded, so that it can be taken out exactly, and
//! what the user changed afterwards is left where it is and said. The old
//! fastctx (0.1.0 to 0.2.6) wrote some of the same places; what it wrote,
//! as its own record says, is taken over or removed, and nothing else.

pub mod apply;
pub mod briefing;
pub mod claude;
pub mod codex;
pub mod direct_firewall;
mod edits;
pub mod executable;
pub mod guidance;
pub mod legacy;
pub mod path_command;
pub mod placement;
pub mod plan;
pub mod program;
pub mod program_collection;
pub mod renewal;
pub mod repair;
#[cfg(windows)]
pub mod interactive;
pub mod server;
pub mod service;
mod source;
pub mod transaction;
pub mod uninstall;
pub mod upgrade;

mod disk;
mod snapshot;

pub use edits::{Error, File, Notice};
pub(crate) use edits::{check_path, finish};
