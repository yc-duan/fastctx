//! What a session says to the user (`world-voice.md`): the `message_user`
//! tool's words, and the files it shows with them.

use serde::{Deserialize, Serialize};

/// The tool's name, as hosts list it.
pub const TOOL: &str = "message_user";

/// The tool's description: all the model is told of when to speak.
pub const DESCRIPTION: &str = "Say something to the user. Only what you send here reaches them; everything else you write is your working notes, which they see only if they open the detailed view. They may be away or asleep; nothing waits for them.

- Each time the user says something to you, answer it here first, with status \"reply\", and then get on with the work: when it takes a while, a sentence on what you will do is enough for now; when a quick look is all the answer needs, take it and answer once. What you write outside this tool is not shown to them.
- Beyond that, speak only when they would want to know: a result they asked for, a real blocker, a decision only they can make. What you say to yourself between steps stays in your notes, and your plan shows where the work stands. \"info\" can wait until they look; \"urgent\" means they should know soon.
- message is Markdown. attachments shows files with it: images, diffs, logs, each on its machine.";

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
