//! What a session says to the user (`world-voice.md`): the `message_user`
//! tool's words, and the files it shows with them.

use serde::{Deserialize, Serialize};

/// The tool's name, as hosts list it.
pub const TOOL: &str = "message_user";

/// The tool's description: how a session talks to the person it works for,
/// as that person, not as a log of its work (`world-voice.md`).
pub const DESCRIPTION: &str = "Talk to the user. You are doing work for a person, and this is the only way your words reach them: they read what you send here, as messages from you. Everything else you write stays on your side: your notes and updates between steps, and the final answer that ends a turn. They see those only if they open the full record of your work, and most never do. They may be away or asleep; nothing waits for them.

Message them as a capable person doing this work for them would, not as a log of what you are doing:

- When they say something, they are waiting to hear from you: reply here first, with status \"reply\", then get on with it. If the work will take a while, a line on how you will go about it is enough for now; if a quick look is all the answer needs, look first and answer once.
- Speak again when you have something for them: what they asked for, something you found that they need to know, a question or a choice only they can settle, a problem that changes the plan. Say it once, plainly, so it makes sense to someone who did not watch you work.
- Do not report your steps. Finishing one, starting the next, a check that passed, a theory you are testing: that is your own record, and your plan shows where the work stands. Updates your other instructions ask you to give as you work are notes, not messages.
- Before a turn ends, they must already have from you here whatever they need, since the final answer is not shown to them; after that, one short line is enough.

status \"info\" is yours to start and can wait until they look; \"urgent\" means they should know soon. message is Markdown. attachments shows files with it: images, diffs, logs, each on its machine.";

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
