//! A conversation's timeline as the content database keeps it: each event
//! of its history an item, so a viewer reads a page of it and is told when
//! it moves, where it once received the whole history.
//!
//! The items are the session's events as interfaces already draw them. They
//! are made by `build`, which reads the history as the World and the host
//! keep it and writes what changed.

pub(crate) mod api;
mod build;
mod identity;
mod order;
pub(crate) mod reads;

#[expect(unused_imports, reason = "the timeline build job is not joined yet")]
pub(crate) use build::{Built, build};

use fastctx_content::StreamKey;
use fastctx_types::ResidentId;

/// The stream a conversation's timeline is kept in.
pub(crate) fn stream(resident: &ResidentId) -> StreamKey {
    StreamKey::new(format!("conversation/{resident}"), "timeline")
}

impl crate::Node {
    /// The content database, when it opened.
    pub(crate) fn content(&self) -> Result<&fastctx_content::Content, fastctx_types::ipc::Failure> {
        self.content
            .as_ref()
            .ok_or_else(|| fastctx_types::ipc::Failure::new("this machine's content database is unavailable"))
    }
}
