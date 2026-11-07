//! What a colleague says to the user (`world-voice.md`, 同事的
//! message_user): the `message_user` tool's words, and the files it shows
//! with them.

use serde::{Deserialize, Serialize};

/// The tool's name, as hosts list it.
pub const TOOL: &str = "message_user";

/// The tool's description as a colleague reads it: the user is the person
/// the agent that asked it works for, whom it answers when they speak to
/// it; what that agent wants goes to it (`world-colleagues.md`, 两个工具).
pub const DESCRIPTION: &str = "Talk to the user: the person the agent that asked you works for. They read what you send here, as messages from you; nothing else you write reaches them, and the last thing you say in a turn goes to the agent that asked you, not to them. Use it to answer the user when they speak to you; what the agent that asked you wants goes to it.

status \"reply\" answers what the user just said; \"info\" can wait until they look; \"urgent\" means they should know soon. message is Markdown. attachments shows files with it: images, diffs, logs, each on its machine.";

/// What the model is told once its words are kept.
pub const TOLD: &str = "=== told the user ===";

/// A file shown with what is said: where it is, as a script names it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
    pub machine: String,
    pub path: String,
}

/// The most files one message shows.
pub const ATTACHMENTS_MOST: usize = 8;

/// What a colleague tells the agent that asked it with `message_user`, as
/// the asker receives it (`world-colleagues.md`, 两个工具): it owes no
/// answer, and it wakes the asker, as the colleague's report does.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColleagueTold {
    pub colleague: crate::Address,
    /// The label of the task it was asked for.
    pub label: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<Attachment>,
}
