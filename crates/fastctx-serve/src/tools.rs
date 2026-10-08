//! The World's tools as a session sees them.

use fastctx_hands::run::{Content, window};
use serde_json::{Value, json};

/// The machines a session works on as its tools were listed: this one, and
/// the others its scope reaches.
pub struct Machines {
    pub here: String,
    pub others: Vec<String>,
}

/// The tools this session lists: the World's shell, and the colleague tools,
/// which a colleague has none of: the World is one level deep
/// (`world-colleagues.md`). Writing files and going to other machines are
/// commands in the shell (`world-run.md`, `world-write.md`). `woken` says
/// whether the World can start a turn here when a job ends or an answer
/// comes; elsewhere they come with the answer of the next World tool call.
pub fn definitions(writes_off: bool, colleague: bool, woken: bool, machines: Option<&Machines>) -> Value {
    let mut tools = vec![run_definition(writes_off, woken, machines)];
    if !colleague {
        tools.push(ask_colleague_definition(woken, machines));
        tools.push(message_colleague_definition());
    }
    for tool in &mut tools {
        // Claude Code counts UTF-16 code units here. One Unicode scalar
        // uses at most two, including every supplementary-plane character.
        tool["_meta"] = json!({ "anthropic/maxResultSizeChars": 2 * window::BUDGET });
    }
    Value::Array(tools)
}

/// The machines there are, when the node could say: a model that knows
/// them reaches the right one by the user's words.
fn machine_names(machines: Option<&Machines>) -> String {
    match machines {
        Some(machines) => {
            let mut names = vec![format!("{} (this one)", machines.here)];
            names.extend(machines.others.iter().cloned());
            format!("The World's machines when this session began: {}.", names.join(", "))
        }
        None => "`fastctx nodes` lists the World's machines.".to_string(),
    }
}

/// `run`'s `cwd`: weaker models reach for it to work on another machine's
/// files, which only `on` does.
fn cwd_description(machines: Option<&Machines>) -> String {
    let here = machines.map(|machines| format!(" ({})", machines.here)).unwrap_or_default();
    format!(
        "The absolute path of a directory on this machine{here} to start in; without it, the home directory. Another machine's directories are not reached through cwd: write `on MACHINE` in the script."
    )
}

/// The `node` parameter of the colleague tool.
fn node(subject: &str, machines: Option<&Machines>) -> Value {
    let description = format!("{subject}, by name; without it, this machine. {}", machine_names(machines));
    json!({ "type": "string", "description": description })
}

fn run_definition(writes_off: bool, woken: bool, machines: Option<&Machines>) -> Value {
    let writing = if writes_off { "" } else { RUN_WRITING };
    let ends = if woken { RUN_ENDS_WOKEN } else { RUN_ENDS_ASKED };
    json!({
        "name": "run",
        "description": format!(
            "{RUN_START}{}\n\n{RUN_REST}{ends}\n\n{RUN_COMMANDS_HEAD}{writing}{RUN_COMMANDS}",
            machine_names(machines)
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The bash script to run, exactly as bash should read it."
                },
                "cwd": {
                    "type": "string",
                    "description": cwd_description(machines)
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

const RUN_START: &str = "\
Run a bash script on this machine and read what it prints. Inside it, `on MACHINE` runs a part of it on another machine of the World, with no address, login or key. ";

const RUN_REST: &str = "\
`on` takes a command and its words, sent exactly as this shell expanded them, or a script on standard input, read only by that machine's bash:
  on laptop cargo test --release
  on hangzhou <<'EOF'
  cd /srv/app && tail -n 200 app.log | grep ERROR
  EOF
It acts as a subshell placed there: the script's own variables, functions and options go along; the machine's own settings, such as PATH, HOME and LANG, are that machine's. Its output, exit code and standard input work as any command's, in pipelines and `if` too. A script written as one quoted word is refused. If the connection drops, it goes on there and its end still reaches you.

Every machine runs bash with the GNU tools and ripgrep (`rg`), Windows ones too (Git Bash), where paths are written like C:/Users/me and `df`, `du`, `ls` and `find` answer what PowerShell would. Git Bash rewrites arguments shaped like Unix paths, such as /FI, before a Windows program sees them: put MSYS_NO_PATHCONV=1 before such a command, and before `cmd /c`, which otherwise silently does nothing.

Each call is a fresh login shell, starting in the home directory unless `cwd` names another; nothing carries over between calls, and standard input is empty. Standard output and standard error arrive together. Images a program draws with the kitty or iTerm2 terminal protocols come back as images. A call waits up to 15 seconds; a script still running then goes on in the background, and the answer names the file its output continues in. ";

const RUN_ENDS_WOKEN: &str = "You are told when it ends.";

const RUN_ENDS_ASKED: &str = "\
After it ends, the answer of your next World tool call says so, with its last lines; until then `fastctx w ID` says whether it still runs.";

const RUN_COMMANDS_HEAD: &str = "The World's commands in this shell; each explains itself with --help:\n";

const RUN_WRITING: &str = "\
- `edit PATH` changes a file by the blocks on its standard input: `<<<<<<< SEARCH`, the exact text now in the file, `=======`, what replaces it, `>>>>>>> REPLACE`. The text must be there exactly once (`--all` replaces every one); all blocks apply, or none. The file keeps its encoding and line endings.
- `write PATH` makes standard input the whole file (`-n` leaves off the final newline).
  Give their text as a quoted here-document, <<'EOF', so bash changes nothing in it; paths are absolute.
";

const RUN_COMMANDS: &str = "\
- `fastctx view PATH...` shows files with line numbers, several at once, images as images; a PDF as its text by page, `--pages 3-5` to choose, `--image` to see them; Word, PowerPoint and Excel as their content, Excel cells by address.
- `fastctx w` shows what is happening in your town now and what happened since your last turn; `fastctx w ID` shows one resident or job.
- `fastctx nodes` lists the machines, which are online and what each runs.
- `fastctx copy MACHINE:PATH MACHINE:PATH` copies files and directories between machines; the destination path is what the copy becomes, and ending it with / copies into that directory.
- `fastctx stop JOB` stops a job and everything it started.
- `fastctx project` lists the towns, each a project, and shows their maps; `fastctx project enter TOWN` moves you into one, only when the user says which project to work on.
- `fastctx journal` reads and adds to your town's journals; `fastctx page` publishes a page, HTML the user opens in the World's app.";

fn ask_colleague_definition(woken: bool, machines: Option<&Machines>) -> Value {
    let delivered = if woken { "" } else { " with the answer of your next World tool call" };
    json!({
        "name": "ask_colleague",
        "description": ASK_COLLEAGUE_DESCRIPTION.replace("{delivered}", delivered),
        "inputSchema": {
            "type": "object",
            "properties": {
                "prompt": {
                    "type": "string",
                    "description": "What you need from the colleague, in your own words. It reaches the colleague word for word, under a line naming you, as its first message."
                },
                "description": {
                    "type": "string",
                    "description": "A label for the task in three to eight words, shown beside the colleague in fastctx w."
                },
                "node": node("The machine the colleague works on", machines)
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

Unlike a subagent of your own, which lives inside this conversation, a colleague lives in the World: it works on whichever machine `node` names, it keeps working if you stop, and everyone in the town can see it with `fastctx w`. The call returns at once with the colleague's id. Each time the colleague ends a turn, its last message is delivered to you{delivered}, under a line saying how the turn ended; that message is its answer, or its question to you. Reply, or give it more, with message_colleague. See what it is doing at any time with `fastctx w <id>`.

Working with a colleague:
- GPT is strong on a part and weak on the whole. Within one part it reads all of the material before acting, works a hard point for hours, changes its approach when one fails, and says plainly when it cannot do something. Asked to judge a whole that has not yet taken shape, or to build a whole thing from nothing, it returns pieces that are careful one by one and have no idea holding them together.
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
