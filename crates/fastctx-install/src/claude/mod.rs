//! Claude's public plugin commands and a reversible edit of its output limit.
//!
//! The caller supplies command execution and fresh file bytes, and owns locking,
//! durable receipts and atomic file publication. Run plugin commands first, then
//! read settings again and call [`settings::apply`] or [`settings::unapply`].
//! Never publish a settings snapshot taken before a plugin command: Claude also
//! edits that file. If file publication fails, roll back the plugin transaction.
//!
//! Integration order: inspect settings and obtain [`plugins::snapshot`], prepare
//! [`plugins::PluginApply`], execute it through [`plugins::PluginHost`], reread
//! settings, then publish the edit and its receipt. On withdrawal run
//! [`plugins::unapply`] before reading settings for [`settings::unapply`]. Keep
//! each receipt bound to the same user/configuration context that created it.
//!
//! Claude's commands are not atomic. In 2.1.287 an install can leave only an
//! enabledPlugins entry, which uninstall cannot address without an install
//! record. In a borrowed marketplace recovery reports this remaining work.
//! Public listings also cannot distinguish identical marketplace recreation
//! or prove ownership after a crash between intent and observed result.

mod jsonc;
mod limit;
pub mod plugins;
pub mod settings;

/// Finds the public CLI on an explicitly supplied PATH, including npm's
/// `claude.cmd` on Windows. No command or host configuration is opened.
pub use fastctx_hosts::claude::background::find;
