//! Claude's public plugin commands and reversible output-limit and deny-rule edits.
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
//! The world's marketplace and plugin are reserved fastctx products. Public
//! state determines their recovery goal even after a command lost its result.
//! An install can leave an enabledPlugins entry without a listable record;
//! registration cleanup removes only this product's declarations in that case.

pub mod consent;
pub mod history;
mod jsonc;
mod limit;
pub mod plugins;
pub(crate) mod process;
pub(crate) mod registration;
pub mod settings;
pub mod tools;

/// Finds the public CLI on an explicitly supplied PATH, including npm's
/// `claude.cmd` on Windows. No command or host configuration is opened.
pub use fastctx_hosts::claude::background::find;
