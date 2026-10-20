//! A native SessionStart receipt survives failure to load the JavaScript mod.

use std::path::PathBuf;

use fastctx_types::layout::ConversationDir;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

/// Claude has reached its native SessionStart hook for this activation.
#[derive(Deserialize, Serialize)]
pub struct Started {
    pub session: String,
    pub at: Timestamp,
    pub process: fastctx_types::ProcessIdentity,
}

/// Input held while a new or resumed host starts without a native prompt.
#[derive(Deserialize, Serialize)]
pub struct Opening {
    pub input: fastctx_types::NoticeId,
    pub at: Timestamp,
    pub previous: Option<fastctx_types::ProcessIdentity>,
}

pub fn opening(directory: &ConversationDir) -> PathBuf {
    directory.path().join("claude-opening.json")
}

pub fn receipt(directory: &ConversationDir) -> PathBuf {
    directory.path().join("claude-started.json")
}
