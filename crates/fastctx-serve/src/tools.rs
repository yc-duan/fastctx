//! The World's tools as a session sees them.

use fastctx_hands::run::{Content, window};
use serde_json::{Value, json};

/// Which way of writing files a session's host is given (`world-write.md`):
/// Codex's patches, or the exact replacements other hosts use. Only one is
/// listed, so a model never weighs two ways of doing one thing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Writer {
    Edit,
    ApplyPatch,
}

impl Writer {
    /// Whether the tool `name` is one this session writes with.
    pub fn lists(self, name: &str) -> bool {
        match self {
            Writer::Edit => name == "edit" || name == "write",
            Writer::ApplyPatch => name == "apply_patch",
        }
    }
}

/// The tools this session lists. A colleague has no colleague tools of its
/// own: the World is one level deep (`world-colleagues.md`).
pub fn definitions(writer: Writer, colleague: bool) -> Value {
    let mut tools = match writer {
        Writer::Edit => vec![run_definition(), edit_definition(), write_definition()],
        Writer::ApplyPatch => vec![run_definition(), apply_patch_definition()],
    };
    if !colleague {
        tools.push(ask_colleague_definition());
        tools.push(message_colleague_definition());
    }
    for tool in &mut tools {
        // Claude Code counts UTF-16 code units here. One Unicode scalar
        // uses at most two, including every supplementary-plane character.
        tool["_meta"] = json!({ "anthropic/maxResultSizeChars": 2 * window::BUDGET });
    }
    Value::Array(tools)
}

const NODE: &str = "The name of the machine whose files to change. Without it, this machine.";

fn run_definition() -> Value {
    json!({
        "name": "run",
        "description": RUN_DESCRIPTION,
        "inputSchema": {
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The bash script to run, exactly as bash should read it."
                },
                "cwd": {
                    "type": "string",
                    "description": "The absolute path of the directory to start in, on the machine the script runs on. Without it the script starts in that machine's home directory."
                },
                "node": {
                    "type": "string",
                    "description": "The name of the machine to run on. Without it the script runs on this machine."
                }
            },
            "required": ["command"],
            "additionalProperties": false
        },
        "annotations": {
            "title": "Run a bash script",
            "readOnlyHint": false,
            "destructiveHint": true,
            "idempotentHint": false,
            "openWorldHint": true
        }
    })
}

const RUN_DESCRIPTION: &str = "\
Run a bash script on a machine of the World and read what it prints.

The script runs on this machine unless `node` names another. Everything in it then happens on that machine: its files, paths, programs and environment, so write paths the way that machine does. The first line of the answer names the machine it ran on.

Each call is a fresh login shell: no directory, variable or function carries over from an earlier call, and nothing is read from standard input, so a program that asks for input fails at once instead of waiting. The script starts in the machine's home directory unless `cwd` names another. Standard output and standard error arrive together, in the order a terminal would show them. Images that a program draws with the kitty or iTerm2 terminal graphics protocols come back as images.

Besides the machine's own programs, the shell has the World's commands, each explained by its `--help`: `fastctx view` shows files, several at once, with line numbers, and images as images; `fastctx w` shows who is in your town and what each is doing; `fastctx project` shows the towns, each a project, and their maps, and `fastctx project enter <town>` moves you into one, only when the user says which project to work on; `fastctx journal` reads and adds to your town's journals; `fastctx nodes` lists the machines and what each is like; `fastctx copy` copies files and directories from one machine to another, written MACHINE:PATH.

A call waits up to 15 seconds. A script still running then goes on in the background; the answer names the file its output continues in, which you can read while it grows. When it ends you are told, with the answer to your next call of a World tool.";

fn edit_definition() -> Value {
    json!({
        "name": "edit",
        "description": EDIT_DESCRIPTION,
        "inputSchema": {
            "type": "object",
            "properties": {
                "file_path": { "type": "string", "description": "The absolute path of the file to change." },
                "old_string": { "type": "string", "description": "The text to replace, exactly as it is in the file." },
                "new_string": { "type": "string", "description": "The text to put in its place." },
                "replace_all": { "type": "boolean", "description": "Replace every occurrence of old_string, not just one. Default false." },
                "node": { "type": "string", "description": NODE }
            },
            "required": ["file_path", "old_string", "new_string"],
            "additionalProperties": false
        },
        "annotations": write_annotations("Edit a file")
    })
}

const EDIT_DESCRIPTION: &str = "\
Replace exact text in a file on a machine of the World.

`old_string` must appear in the file exactly as given, whitespace and indentation included, and only once: include enough surrounding lines to make it unique, or set `replace_all` to replace every occurrence. The file keeps its encoding and line endings. `file_path` is absolute, written the way the machine `node` names writes paths. The answer shows the changed lines with their line numbers.";

fn write_definition() -> Value {
    json!({
        "name": "write",
        "description": WRITE_DESCRIPTION,
        "inputSchema": {
            "type": "object",
            "properties": {
                "file_path": { "type": "string", "description": "The absolute path of the file to create or replace." },
                "content": { "type": "string", "description": "The file's whole new contents." },
                "node": { "type": "string", "description": NODE }
            },
            "required": ["file_path", "content"],
            "additionalProperties": false
        },
        "annotations": write_annotations("Write a file")
    })
}

const WRITE_DESCRIPTION: &str = "\
Create a file, or replace a file's whole contents, on a machine of the World.

To change part of a file, use `edit`. A file you looked at with `fastctx view` that has changed since is not overwritten, so nobody's change is lost unseen: look at it again first. Missing parent directories are created. `file_path` is absolute, written the way the machine `node` names writes paths.";

fn apply_patch_definition() -> Value {
    json!({
        "name": "apply_patch",
        "description": APPLY_PATCH_DESCRIPTION,
        "inputSchema": {
            "type": "object",
            "properties": {
                "input": { "type": "string", "description": "The whole patch, from *** Begin Patch to *** End Patch." },
                "node": { "type": "string", "description": NODE }
            },
            "required": ["input"],
            "additionalProperties": false
        },
        "annotations": write_annotations("Apply a patch")
    })
}

const APPLY_PATCH_DESCRIPTION: &str = "\
Edit files on a machine of the World with a patch.

*** Begin Patch
*** Update File: /home/me/app/greet.py
@@ def greet():
-    print(\"Hi\")
+    print(\"Hello, world!\")
*** Add File: /home/me/app/notes.txt
+first line
*** Delete File: /home/me/app/old.txt
*** End Patch

Each file's section starts with `*** Add File:`, `*** Delete File:` or `*** Update File:`, which may be followed by `*** Move to:` and a new path. In an update, each hunk starts with `@@`, optionally followed by a line that locates it, such as the function it is in; its lines start with a space for context, `-` for a line removed and `+` for a line added. Give about three lines of context around each change. Every new file's lines start with `+`.

Every path is absolute, written the way the machine `node` names writes paths; a relative path is refused. Files keep their encoding and line endings. The answer lists what changed: A added, M modified, D deleted.";

fn ask_colleague_definition() -> Value {
    json!({
        "name": "ask_colleague",
        "description": ASK_COLLEAGUE_DESCRIPTION,
        "inputSchema": {
            "type": "object",
            "properties": {
                "prompt": {
                    "type": "string",
                    "description": "What you need from the colleague, in your own words. It is delivered word for word as its first message."
                },
                "description": {
                    "type": "string",
                    "description": "A label for the task in three to eight words, shown beside the colleague in fastctx w."
                },
                "node": {
                    "type": "string",
                    "description": "The name of the machine the colleague works on. Without it, this machine."
                }
            },
            "required": ["prompt", "description"],
            "additionalProperties": false
        },
        "annotations": {
            "title": "Ask a colleague",
            "readOnlyHint": false,
            "destructiveHint": false,
            "idempotentHint": false,
            "openWorldHint": true
        }
    })
}

const ASK_COLLEAGUE_DESCRIPTION: &str = "\
Ask a colleague to take on a part of the work: a GPT resident of the World, started through Codex, which works on its own while you go on.

Unlike a subagent of your own, which lives inside this conversation, a colleague lives in the World: it works on whichever machine `node` names, it keeps working if you stop, and everyone in the town can see it with `fastctx w`. The call returns at once with the colleague's id. Each time the colleague ends a turn, its last message is delivered to you, under a line saying how the turn ended; that message is its answer, or its question to you. Reply, or give it more, with message_colleague. See what it is doing at any time with `fastctx w <id>`.

Working with a colleague:
- GPT is strong on a part and weak on the whole. Within one part it reads all of the material before acting, works a hard point for hours, changes its approach when one fails, and says plainly when it cannot do something. Asked to judge a whole that has not yet taken shape, or to build a whole thing from nothing, it returns pieces that are careful one by one and have no idea holding them together. Keep the whole yourself, and give it parts.
- Build the whole yourself, and hand out the parts as they appear: a piece of code whose need can be stated and whose result facts can judge, something that must be airtight, a behaviour nobody understands yet, a detail that must be exactly right, an exhaustive check of an outside fact such as an interface or a program's real behaviour, a counterexample to make, an experiment or a measurement that answers a definite question. The moment you are about to read through a pile of material, try a set of cases or make something airtight is the moment to ask. That you could do it in a few minutes is no reason to keep it: done in minutes, it is often done shallowly.
- Do not use it mainly to review what you made; that is its weakest use. Give it things to build, find out or measure.
- Say what this part needs and why, and where things stand. Point at the material rather than summarizing it. Do not fence it in with which files it may touch or which checks it must pass, and do not tell it how to work: that is where it is strongest. Tell it the rules it must keep: what must not be broken, and the limits of anything shared it will touch, such as a server's memory.
- To learn a fact, ask a question that facts answer, not for its opinion of a plan.
- When it reports, check the claims you will build on. If it answers a different question than the one you asked, ask again: does it hold, yes or no, and what is the evidence.
- One part, one colleague; several can work at once. Colleagues that might touch the same files each work on their own copy, such as their own git worktree, and you merge.";

fn message_colleague_definition() -> Value {
    json!({
        "name": "message_colleague",
        "description": MESSAGE_COLLEAGUE_DESCRIPTION,
        "inputSchema": {
            "type": "object",
            "properties": {
                "to": {
                    "type": "string",
                    "description": "The colleague's id as ask_colleague or fastctx w gave it, such as codex-7f3a. Write codex-7f3a@laptop to name the machine it lives on."
                },
                "message": {
                    "type": "string",
                    "description": "What to tell it, delivered word for word."
                }
            },
            "required": ["to", "message"],
            "additionalProperties": false
        },
        "annotations": {
            "title": "Message a colleague",
            "readOnlyHint": false,
            "destructiveHint": false,
            "idempotentHint": false,
            "openWorldHint": true
        }
    })
}

const MESSAGE_COLLEAGUE_DESCRIPTION: &str = "\
Send words to a colleague: one you asked for, or any other in the town, including one that has gone to sleep.

If it is working, the words join its current turn at its next tool call; if it is idle or asleep, they start a new turn. Either way, the last message of the turn they go into is delivered to you. A colleague on another machine gets them as soon as its machine is reachable.";

fn write_annotations(title: &str) -> Value {
    json!({
        "title": title,
        "readOnlyHint": false,
        "destructiveHint": true,
        "idempotentHint": false,
        "openWorldHint": false
    })
}

/// A tool's answer as MCP carries it.
pub fn result(content: &[Content], is_error: bool) -> Value {
    let blocks: Vec<Value> = content
        .iter()
        .map(|block| match block {
            Content::Text(text) => json!({ "type": "text", "text": text }),
            Content::Image { mime, base64 } => {
                json!({ "type": "image", "data": base64, "mimeType": mime })
            }
        })
        .collect();
    json!({ "content": blocks, "isError": is_error })
}
