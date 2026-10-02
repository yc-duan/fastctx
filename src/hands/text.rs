//! Every sentence a model reads about the World's hands: tool descriptions, parameter notes,
//! head notes and notices. Kept in one place because each word here is read by models on two
//! hosts, and a route is learned from what co-occurs, so a name appears only where its tool or
//! command really exists (`specs/world-run.md`, `world-view.md`, `world-write.md`,
//! `world-colleagues.md`).

/// The seconds `run` waits before a command continues in the background. Set by the World,
/// never by the model: it bounds how late a message to a busy resident can be, because both
/// hosts read incoming words at tool boundaries (`specs/world-run.md`, "一次调用等 15 秒").
pub(crate) const RUN_WAIT_SECONDS: u64 = 15;

pub(crate) const RUN_DESCRIPTION: &str = concat!(
    "Run a bash command on this machine or, with node, on another machine of this World. ",
    "Each call is a fresh bash: nothing carries over from earlier calls, stdin is empty, and it ",
    "starts in that machine's home unless cwd names an absolute directory. A command still running ",
    "after 15 seconds keeps running in the background: you get its output so far, its id, and the ",
    "file the rest of its output goes to, and you will be told when it ends, so there is no need to ",
    "wait or poll. Look at files with `fastctx view <paths>`: several at once, line or page ranges, ",
    "images and PDF pages come back to you as images. Search with rg, find and ls. `fastctx w` shows ",
    "who is in this town and what they are doing, and `fastctx w <id>` one resident or one running ",
    "command. Images a program prints with the iTerm2 or kitty image protocol come back as images. ",
    "To stop a background command, kill the process id that `fastctx w <id>` shows."
);

pub(crate) const RUN_COMMAND: &str = "The bash script to run, exactly as you would type it in a terminal.";
pub(crate) const RUN_CWD: &str = "Absolute directory to start in. Omitted: that machine's home.";
pub(crate) const NODE: &str =
    "Machine to act on, by name. Omitted: this machine. The first line of the result names the machine it happened on.";

pub(crate) const EDIT_DESCRIPTION: &str = concat!(
    "Replace exact text in a file on this machine or, with node, on another machine of this ",
    "World. old_string must appear in the file exactly once; include more surrounding lines to ",
    "make it unique, or set replace_all to change every occurrence. The path is absolute. The ",
    "file keeps its encoding, byte order mark and line endings. Returns the changed lines with ",
    "their line numbers."
);
pub(crate) const EDIT_PATH: &str = "Absolute path of the file to change.";
pub(crate) const EDIT_OLD: &str = "The exact text to replace, including indentation.";
pub(crate) const EDIT_NEW: &str = "The text to put in its place.";
pub(crate) const EDIT_ALL: &str = "Replace every occurrence instead of requiring exactly one.";

pub(crate) const WRITE_DESCRIPTION: &str = concat!(
    "Create a file, or replace a whole file, on this machine or, with node, on another machine ",
    "of this World. The path is absolute and missing parent directories are created. If you ",
    "viewed this file earlier in this conversation and it has changed since, the write is ",
    "refused, so nobody's changes are silently lost; view it again first. To change part of a ",
    "file, use edit."
);
pub(crate) const WRITE_PATH: &str = "Absolute path of the file to create or replace.";
pub(crate) const WRITE_CONTENT: &str = "The complete new content of the file.";

pub(crate) const APPLY_PATCH_DESCRIPTION: &str = concat!(
    "Apply a patch to files on this machine or, with node, on another machine of this World. ",
    "The patch uses the apply_patch format: `*** Begin Patch`, then `*** Add File: <path>`, ",
    "`*** Delete File: <path>` or `*** Update File: <path>` sections (an update may carry ",
    "`*** Move to: <path>` and `@@` hunks with context lines), then `*** End Patch`. Every path ",
    "is absolute. Files keep their encoding and line endings. Returns the files it changed."
);
pub(crate) const APPLY_PATCH_INPUT: &str = "The whole patch, from *** Begin Patch to *** End Patch.";

pub(crate) const ASK_COLLEAGUE_DESCRIPTION: &str = concat!(
    "Ask a colleague, a GPT resident of this World, to take on a piece of work. Your own ",
    "subagents live inside this conversation; a colleague lives in the World: it works on the ",
    "machine you name, keeps working after you stop, and everyone in the town can see what it does. ",
    "It starts at once and works in the background; this call returns its id right away. The last ",
    "message of each of its turns is delivered to you word for word: when it finishes, that is its ",
    "result; when it stops to ask you something, that is its question, and you answer with ",
    "message_colleague.\n\n",
    "Getting the most from it: GPT is strong at how (reading everything first, grinding a hard ",
    "problem for hours, getting every corner right, checking until it holds) and weak when asked to ",
    "judge. Keep the what and the why with you and hand it the how. Give it a goal it can check for ",
    "itself, a visible end state, and set it high. Write what you understand as constraints, must and ",
    "must not, rather than as reasons to absorb; point it at specs and code by absolute path instead ",
    "of summarizing them. Never prescribe the method. Ask it to do things, not to evaluate them, and ",
    "phrase questions so that facts can answer them. While it works, do not interrupt unless the ",
    "premise changed; `fastctx w <id>` shows what it is actually doing. When it reports back, look at ",
    "what it really ran before believing \"done\", and if it answers a different question than you ",
    "asked, ask again whether your question holds and on what evidence."
);
pub(crate) const ASK_PROMPT: &str = "Everything the colleague needs, in your own words; delivered verbatim.";
pub(crate) const ASK_DESCRIPTION: &str = "A three-to-eight-word label for the work, shown next to its name in fastctx w.";
pub(crate) const ASK_NODE: &str = "Machine it works on. Omitted: this machine.";

pub(crate) const MESSAGE_COLLEAGUE_DESCRIPTION: &str = concat!(
    "Speak to a colleague, or to any resident, by its id. If it is working, your words join its ",
    "current turn at its next tool boundary; if it is idle or asleep, they start a new turn. The ",
    "last message of the turn your words went into is delivered to you. `fastctx w -a` lists ",
    "residents including earlier colleagues you can wake."
);
pub(crate) const MESSAGE_TO: &str = "The resident's id, such as codex-7f3a.";
pub(crate) const MESSAGE_TEXT: &str = "What you want to say; delivered verbatim with your name.";

/// First line of a `run` result that finished within the wait.
pub(crate) fn run_finished_head(machine: &str, exit: &str) -> String {
    format!("=== run on {machine} ({exit}) ===")
}

/// First line of a `run` result that continues in the background.
pub(crate) fn run_continuing_head(machine: &str, job: &str) -> String {
    format!("=== run on {machine} (still running after {RUN_WAIT_SECONDS}s; {job}) ===")
}

/// Last line of a `run` result that continues in the background.
pub(crate) fn run_continuing_tail(machine: &str, log: &str) -> String {
    format!("=== output continues in {machine} {log}; you'll be told when it ends ===")
}

/// The notice a resident receives when a command it started has ended.
pub(crate) fn job_ended_notice(job: &str, machine: &str, ended: &str, elapsed: &str, command: &str, log: &str) -> String {
    format!("=== {job} on {machine} {ended} after {elapsed}: {command} ===\noutput: {machine} {log}")
}

/// What `ask_colleague` returns.
pub(crate) fn colleague_started(id: &str, machine: &str) -> String {
    format!(
        "=== colleague {id} on {machine} (started) ===\nIts last message of each turn will be delivered to you. See what it is doing: fastctx w {id}"
    )
}

/// What `message_colleague` returns.
pub(crate) fn message_landed(id: &str, machine: &str, how: &str) -> String {
    format!("=== message to {id} on {machine} ({how}) ===")
}
