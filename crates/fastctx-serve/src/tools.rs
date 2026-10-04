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

/// The tools this session lists.
pub fn definitions(writer: Writer) -> Value {
    match writer {
        Writer::Edit => json!([run_definition(), edit_definition(), write_definition()]),
        Writer::ApplyPatch => json!([run_definition(), apply_patch_definition()]),
    }
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
        },
        "_meta": { "anthropic/maxResultSizeChars": window::BUDGET }
    })
}

const RUN_DESCRIPTION: &str = "\
Run a bash script on a machine of the World and read what it prints.

The script runs on this machine unless `node` names another. Everything in it then happens on that machine: its files, paths, programs and environment, so write paths the way that machine does. The first line of the answer names the machine it ran on.

Each call is a fresh login shell: no directory, variable or function carries over from an earlier call, and nothing is read from standard input, so a program that asks for input fails at once instead of waiting. The script starts in the machine's home directory unless `cwd` names another. Standard output and standard error arrive together, in the order a terminal would show them. Images that a program draws with the kitty or iTerm2 terminal graphics protocols come back as images.

Besides the machine's own programs, the shell has the World's commands, each explained by its `--help`: `fastctx view` shows files, several at once, with line numbers, and images as images; `fastctx w` shows who is here and what each is doing; `fastctx nodes` lists the machines and what each is like.

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
