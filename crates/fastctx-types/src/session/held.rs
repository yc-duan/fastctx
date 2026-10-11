//! What a viewing device keeps of a session's history, and what the machine
//! the session lives on sends it in place of the whole history
//! (`world-gui.md`, 对话: every device that views a conversation keeps it).
//!
//! The machine a session lives on cuts its history, as an interface draws
//! it, into segments named by their content, and says which segments in
//! which order make up the history now (its layout). A viewer says which
//! segments it keeps; only the others come. A host that rewrote its history
//! (a rollback, a compaction, a fork) makes new segments where the old ones
//! no longer match, and only those are sent again. The machine the session
//! lives on keeps nothing for its viewers: it reads its records again each
//! time, and a version it names lets it answer that nothing changed without
//! reading them whole.

use serde::{Deserialize, Serialize};

use super::SessionEvent;

/// A segment of a history, named by its content: the first 16 bytes of a
/// SHA-256, in lowercase hex.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(into = "String", try_from = "String")]
pub struct SegmentId(String);

impl From<SegmentId> for String {
    fn from(id: SegmentId) -> Self {
        id.0
    }
}

impl TryFrom<String> for SegmentId {
    type Error = String;

    fn try_from(text: String) -> Result<Self, String> {
        Self::parse(&text).ok_or_else(|| format!("{text:?} is not a segment's name"))
    }
}

impl SegmentId {
    /// The name of content whose SHA-256 is `digest`.
    pub fn of(digest: &[u8; 32]) -> Self {
        Self(digest[..16].iter().map(|byte| format!("{byte:02x}")).collect())
    }

    /// The name as written; a name that came from elsewhere is checked to
    /// be one, so it can name a file.
    pub fn parse(text: &str) -> Option<Self> {
        (text.len() == 32 && text.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f')))
            .then(|| Self(text.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SegmentId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// What a viewer holds of a session's history, said when it starts to
/// watch. Saying it at all asks for the history as segments, slimmed to
/// what an interface draws, the images in it as references.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Held {
    /// The version of the history the viewer holds, as the session's
    /// machine named it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// The segments it holds, in the order it holds them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub segments: Vec<SegmentId>,
    /// Holding nothing, it would have the latest part first and the rest
    /// after it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub latest_first: bool,
}

/// One segment's events, in order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Segment {
    pub id: SegmentId,
    pub events: Vec<SessionEvent>,
}

/// Bytes named by their content: how many, and their SHA-256 in lowercase
/// hex.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Fingerprint {
    pub size: u64,
    pub sha256: String,
}
