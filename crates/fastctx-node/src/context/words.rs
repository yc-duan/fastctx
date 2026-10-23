//! Model-facing context wording, kept together for editorial changes.

pub(super) fn changed(name: &str, by: &str) -> String {
    format!("{by} changed the {name} context; this replaces the previous version.")
}

pub(super) fn restored(name: &str) -> String {
    format!("The current {name} context replaces the version in your opening instructions.")
}

pub(super) fn removed(name: &str) -> String {
    format!("The {name} context is now empty; the earlier version no longer applies.")
}

pub(super) fn truncated(shown: usize, total: usize) -> String {
    format!("\n[Context truncated: the first {shown} of {total} UTF-8 bytes are shown.]")
}

pub(super) fn heading(name: &str, text: &str) -> String {
    format!("=== {name} ===\n{text}")
}

pub(super) const PLAN_UNAVAILABLE: &str = "The plan is unavailable.";

pub(super) const USER: &str = "The user";
pub(super) const UNKNOWN: &str = "unknown conversation";
pub(super) const UNAVAILABLE: &str = "the conversation is unavailable";
pub(super) const INVALID_NAME: &str = "invalid context block name";
pub(super) const TOO_MANY: &str = "a conversation can keep at most eight context blocks";
pub(crate) const REVISION_EXHAUSTED: &str = "context revision exhausted";
pub(super) const EPOCH_EXHAUSTED: &str = "context epoch exhausted";
pub(crate) const CODEX_PENDING: &str = "Codex has not made this thread's persisted context available";
pub(crate) fn codex_read(error: impl std::fmt::Display) -> String {
    format!("reading Codex compactions: {error}")
}
