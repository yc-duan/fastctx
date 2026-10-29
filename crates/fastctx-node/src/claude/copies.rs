//! Copies Claude Code starts instead of waking a session.
//!
//! Asked to resume a background session that is still running, Claude Code
//! starts a copy of it under a new id (`fastctx_hosts::claude::background`):
//! the whole history, and none of the World's settings, so nothing names its
//! conversation, whom it works for, or what an assisted session may not use.
//! The World stops and removes such a copy as soon as Claude Code names it,
//! and no copy pairs with a conversation: the words it was given are the
//! woken session's, delivered to that one again.

use std::collections::HashSet;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::Node;

/// The longest a session reporting to the node waits for wakes in flight:
/// a wake's own command is cut off well before (`claude::wake`).
const WAKES_WITHIN: Duration = Duration::from_secs(150);

pub(crate) struct Copies {
    /// The short ids Claude Code printed for the copies stopped.
    stopped: Mutex<HashSet<String>>,
    /// How many wakes are in flight. A session that reports meanwhile, and
    /// names no conversation, waits for them, so that a copy is known as one
    /// before it could pair.
    waking: tokio::sync::watch::Sender<usize>,
}

impl Default for Copies {
    fn default() -> Self {
        Self { stopped: Mutex::default(), waking: tokio::sync::watch::Sender::new(0) }
    }
}

impl Copies {
    /// Counts a wake in flight until the returned guard is dropped.
    pub(crate) fn waking(&self) -> Waking<'_> {
        self.waking.send_modify(|count| *count += 1);
        Waking { copies: self }
    }

    /// Waits until no wake is in flight, or until a wake could not still be.
    pub(crate) async fn settled(&self) {
        let mut wakes = self.waking.subscribe();
        let _ = tokio::time::timeout(WAKES_WITHIN, wakes.wait_for(|count| *count == 0)).await;
    }

    /// Whether `session` is a copy the World stopped.
    pub(crate) fn is_copy(&self, session: &str) -> bool {
        self.stopped.lock().expect("never poisoned").iter().any(|short| session.starts_with(short.as_str()))
    }
}

pub(crate) struct Waking<'a> {
    copies: &'a Copies,
}

impl Drop for Waking<'_> {
    fn drop(&mut self) {
        self.copies.waking.send_modify(|count| *count = count.saturating_sub(1));
    }
}

/// The copy Claude Code printed `printed` for, when it started one instead
/// of resuming `session`.
pub(crate) fn of(session: &str, printed: &str) -> Option<String> {
    fastctx_hosts::claude::background::started(printed).filter(|short| !session.starts_with(short.as_str()))
}

/// Stops the copy `short` of the login `home` and removes Claude Code's
/// record of it. It is refused as a copy from now on, whether or not the
/// commands succeed; a failure is only logged, as the copy cannot act
/// through the World either way.
pub(crate) async fn discard(node: &Arc<Node>, home: Option<&Path>, short: &str) {
    node.claude.copies.stopped.lock().expect("never poisoned").insert(short.to_string());
    node.log
        .line(format_args!("Claude Code started the copy {short} instead of waking a running session; stopping it"));
    for command in ["stop", "rm"] {
        if let Err(error) = super::management::run(node, home, vec![command.into(), short.into()]).await {
            node.log.line(format_args!("{command} of the Claude copy {short}: {}", error.message));
        }
    }
}
