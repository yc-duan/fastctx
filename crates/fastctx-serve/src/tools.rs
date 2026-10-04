//! The World's tools as a session sees them.

use fastctx_hands::run::{Content, window};
use serde_json::{Value, json};

/// The tools this session lists.
pub fn definitions() -> Value {
    json!([run_definition()])
}

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
