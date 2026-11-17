//! Public, host-independent item bodies. Timeline pages inline the bounded V1
//! descriptor; expanding a part uses a range read, never the host's original JSON.
use super::PictureTarget;
use crate::sync::{
    conversation_identity::ImageSelector,
    cursor::{ByteCount, Count},
    manifest::{ContentRef, ManifestRoot},
};
use serde::{Deserialize, Serialize};

pub const DISPLAY_VERSION: u16 = 1;
pub const DISPLAY_BYTES: usize = 16 * 1024;
pub const DISPLAY_PREVIEW_BYTES: usize = 4 * 1024;
pub const DISPLAY_LABEL_BYTES: usize = 256;
pub const DISPLAY_SUMMARY_BYTES: usize = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplayKind {
    User,
    Message,
    Thinking,
    Plan,
    Tool,
    System,
    Compaction,
    Agent,
    Hidden,
    Unknown,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplayStatus {
    Pending,
    Running,
    Complete,
    Failed,
    Interrupted,
    Declined,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionKind {
    Command,
    Edit,
    Read,
    Search,
    Image,
    Colleague,
    Agent,
    Call,
}

/// Extracted at ingestion/merge time. These are bounded display summaries, not
/// complete command arguments or paths to use as authorities for later actions.
/// Producers truncate text/command/path with display_preview to at most 1 KiB.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionSummary {
    pub kind: ActionKind,
    pub text: Option<String>,
    pub command: Option<String>,
    pub path: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// Producers truncate name/server/machine to at most 256 UTF-8 bytes each.
pub struct ToolSummary {
    pub name: String,
    pub server: Option<String>,
    pub machine: Option<String>,
    pub does: ActionSummary,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplayChannel {
    Text,
    Thinking,
    Plan,
    ToolInput,
    ToolOutput,
    Image,
    Audio,
    Video,
    File,
}
impl DisplayChannel {
    pub fn is_text(self) -> bool {
        matches!(self, Self::Text | Self::Thinking | Self::Plan | Self::ToolInput | Self::ToolOutput)
    }
    pub fn accepts_append(self, channel: crate::session::Part) -> bool {
        match channel {
            crate::session::Part::Text => matches!(self, Self::Text | Self::Thinking | Self::Plan),
            crate::session::Part::Output => self == Self::ToolOutput,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisplayPicture {
    pub selector: ImageSelector,
    pub target: PictureTarget,
}

/// Text channels contain UTF-8; ToolInput contains UTF-8 JSON. Media contains
/// original bytes. `preview` is a UTF-8 prefix, truncated at a character boundary;
/// it is not a second body. The producer verifies the prefix against content.
/// Parts keep their zero-based order across inline and overflow pages. Producers
/// truncate media_type to at most 256 UTF-8 bytes. For a Manifest content reference
/// the writer verifies bytes against its total decoded extent; types can verify
/// that equality only for Single references.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisplayPart {
    pub index: Count,
    pub channel: DisplayChannel,
    pub content: ContentRef,
    pub bytes: ByteCount,
    pub preview: Option<String>,
    pub media_type: Option<String>,
    pub picture: Option<DisplayPicture>,
}
/// Selects one exact growing part, including when it is in an overflow page.
/// BodySnapshot.text_part/text_bytes describe this part, not a whole descriptor.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisplayAppend {
    pub channel: crate::session::Part,
    pub index: Count,
}

/// The overflow descriptors in index order: one DisplayPartsV1 page, or a
/// Branch over such pages keyed by part index as 16 lowercase hex digits. Each
/// page references exactly the bodies its parts own, so the tree retains what
/// it describes; a lightweight replica fetches its pages without those bodies.
/// The writer validates exact contiguous coverage.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisplayPartsRef {
    pub root: ManifestRoot,
    pub start: Count,
    pub count: Count,
}

/// The only public body descriptor for GUI, mobile and export. Unknown host JSON
/// belongs exclusively in BodySnapshot.original. Use encode/decode to enforce the
/// 16 KiB encoded bound and 4 KiB aggregate preview budget before publication.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ItemDisplayV1 {
    pub version: u16,
    pub kind: DisplayKind,
    pub tool: Option<ToolSummary>,
    pub status: Option<DisplayStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facts: Option<Box<super::DisplayFactsV1>>,
    pub append: Option<DisplayAppend>,
    pub parts: Vec<DisplayPart>,
    pub more: Option<DisplayPartsRef>,
}
/// One overflow leaf. Previews are absent here, so the entire item's preview
/// budget is enforced by its root descriptor without scanning all parts.
/// Each leaf obeys the same encoded 16 KiB bound and never carries host JSON.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ItemDisplayPartsV1 {
    pub version: u16,
    pub start: Count,
    pub parts: Vec<DisplayPart>,
}

/// The logical Content response for a Branch of overflow descriptors. Child
/// coordinates are part indexes, and each child is fetched through Content again.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ItemDisplayPartsBranchV1 {
    pub version: u16,
    pub start: Count,
    pub count: Count,
    pub children: Vec<DisplayPartsRef>,
}

/// A byte budget still ends at a Unicode scalar boundary. The caller shares the
/// 4 KiB budget across parts; JSON escaping is charged by descriptor encoding.
pub fn display_preview(text: &str, bytes: usize) -> String {
    let mut end = bytes.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}
