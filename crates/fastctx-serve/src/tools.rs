//! The World's tools as a session sees them.
//!
//! Everything here is read by a model. It says what the model's working
//! environment is and how to act in it, in words the model already knows:
//! the user's machines, projects, sessions, colleagues, jobs. The design's
//! own words (World, resident, town) stay out.

use fastctx_hands::run::{Content, window};
use serde_json::{Value, json};

/// The machines a session works on as its tools were listed: this one, and
/// the others its scope reaches.
pub struct Machines {
    pub here: String,
    pub others: Vec<String>,
}

/// The tools this session lists: the shell, and the colleague tools, which a
/// colleague has none of: the World is one level deep
/// (`world-colleagues.md`). Writing files and going to other machines are
/// commands in the shell (`world-run.md`, `world-write.md`). `woken` says
/// whether the World can start a turn here when a job ends or an answer
/// comes; elsewhere they come with the answer of the next tool call.
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
            format!("The user's machines, connected by fastctx: {}.", names.join(", "))
        }
        None => "The user's machines are connected by fastctx; `fastctx nodes` lists them.".to_string(),
    }
}

/// `run`'s `cwd`: weaker models reach for it to work on another machine's
/// files, which only `on` does.
fn cwd_description(machines: Option<&Machines>) -> String {
    let here = machines.map(|machines| format!(" ({})", machines.here)).unwrap_or_default();
    format!(
        "Absolute path of a directory on this machine{here} to start in; without it, the home directory. For a directory on another machine, write `on MACHINE` in the script and `cd` there."
    )
}

fn run_definition(writes_off: bool, woken: bool, machines: Option<&Machines>) -> Value {
    let writing = if writes_off { "" } else { RUN_WRITING };
    let ends = if woken { RUN_ENDS_WOKEN } else { RUN_ENDS_ASKED };
    json!({
        "name": "run",
        "description": format!(
            "{} {RUN_START}\n\n{RUN_SHELL}{ends}\n\n{RUN_FILES}{writing}{RUN_COPY}\n\n{RUN_ON}\n\n{RUN_COMMANDS}",
            machine_names(machines)
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The bash script to run."
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
This tool is your shell on all of them: it runs a bash script on this machine, and inside the script `on MACHINE` runs a part of it on another machine, with no address, login or key.";

const RUN_SHELL: &str = "\
Shell:
- Every machine runs bash with the GNU tools and `rg`, Windows ones too (Git Bash). Write Windows paths like C:/Users/me, and use `ls`, `find`, `du` and `df` rather than PowerShell.
- Git Bash rewrites arguments that look like Unix paths, such as /FI, before a Windows program sees them. Put MSYS_NO_PATHCONV=1 before such a command, and before `cmd /c`, which otherwise silently does nothing.
- Each call is a fresh login shell, starting in the home directory, or in `cwd`. Nothing carries over between calls, and standard input is empty. Standard output and standard error come back together.
- Images a program draws with the kitty or iTerm2 terminal protocols come back as images.
- A call waits up to 15 seconds. A script still running then goes on in the background, and the answer names the file its output continues in. ";

const RUN_ENDS_WOKEN: &str = "You are told when it ends; meanwhile you can do other work, or end your turn.";

const RUN_ENDS_ASKED: &str = "\
When it has ended, the answer of your next call to this tool says so, with its last lines; until then `fastctx w ID` says whether it still runs.";

const RUN_FILES: &str = "\
Files:
- `fastctx view PATH...` shows files: text with line numbers, several at once; images as images; a PDF's text by page (`--pages 3-5`; `--image` to see the pages); Word, PowerPoint and Excel content, Excel cells by address. Use it for any file you need to look at.
";

const RUN_WRITING: &str = "\
- `edit PATH` changes a file with SEARCH/REPLACE blocks on standard input:
    edit /srv/app/app.ini <<'EOF'
    <<<<<<< SEARCH
    port = 8080
    =======
    port = 8081
    >>>>>>> REPLACE
    EOF
  The SEARCH text must be in the file exactly once (`--all` replaces every one); all blocks apply, or none. The file keeps its encoding and line endings.
- `write PATH` makes standard input the whole file (`-n`: no final newline).
- `edit` and `write` take absolute paths. Quote the here-document delimiter, <<'EOF', so bash leaves the text as written.
";

const RUN_COPY: &str = "\
- `fastctx copy MACHINE:PATH MACHINE:PATH` copies files and directories between machines. The destination path is what the copy becomes; end it with / to copy into that directory.";

const RUN_ON: &str = "\
Other machines: write `on MACHINE` before a command, or give it a script on standard input:
    on laptop cargo test --release
    on server <<'EOF'
    cd /srv/app && tail -n 200 app.log | grep ERROR
    EOF
- The part sent there sees your script's variables, functions and shell options; PATH, HOME, LANG and the machine's other settings are its own.
- Its output, exit code and standard input work like any command's, in pipelines and `if` too.
- A script quoted as one word, such as `on server 'cd /srv && ls'`, is refused: use a here-document.
- If the connection drops, it goes on there, and its end still reaches you.
- Every command here works there too, such as `on server fastctx view /srv/app/app.ini`.";

const RUN_COMMANDS: &str = "\
More commands (each explains itself with --help):
- `fastctx nodes` lists the machines, which are online, and what each runs.
- `fastctx w` shows what runs and who works now, and what happened since your last turn; `fastctx w ID` shows one session or job.
- `fastctx stop JOB` stops a background job and everything it started.
- `fastctx project` lists the user's projects and shows their maps. A fastctx project is not a folder or a repository but a named piece of work that can span several machines; its map says what it is for and where its code and data are on each. `fastctx project enter NAME` makes you work in one; enter one only when the user tells you which project to work on.
- `fastctx journal` reads and adds to the project's journals, the history of its work. `fastctx page` publishes an HTML page the user opens in the World app.";

fn ask_colleague_definition(woken: bool, machines: Option<&Machines>) -> Value {
    let delivered = if woken { "" } else { " with the answer of your next call to a fastctx tool" };
    let node = format!("The machine the colleague works on; without it, this one. {}", machine_names(machines));
    json!({
        "name": "ask_colleague",
        "description": ASK_COLLEAGUE_DESCRIPTION.replace("{delivered}", delivered),
        "inputSchema": {
            "type": "object",
            "properties": {
                "prompt": {
                    "type": "string",
                    "description": "What you need from the colleague, in your own words. It becomes the colleague's first message, word for word, under a line naming you."
                },
                "description": {
                    "type": "string",
                    "description": "A label for the task in three to eight words, shown beside the colleague in `fastctx w`."
                },
                "node": { "type": "string", "description": node }
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
Start a colleague on a part of the work: a GPT agent, run by Codex, that works on its own while you go on. The call returns its id at once.

A colleague is not a subagent inside this conversation. It works on whichever of the user's machines `node` names, it keeps working if you stop, and `fastctx w ID` shows what it is doing. Each time it ends a turn, its last message is delivered to you{delivered}, under a line saying how the turn ended: that message is its answer, or its question to you. Reply, or give it more, with message_colleague.

When to ask one:
- GPT is strong on a part and weak on the whole. Within one part it reads all the material before acting, works a hard point for hours, changes approach when one fails, and says plainly when it cannot do something. Asked to judge a whole that has not taken shape, or to build a whole thing from nothing, it returns pieces that are each careful and have no idea holding them together.
- So keep the whole yourself, and hand out parts as they come up: code whose need you can state and whose result facts can judge; something that must be airtight; a behavior nobody understands yet; a detail that must be exactly right; an exhaustive check of an outside fact, such as an interface or a program's real behavior; a counterexample to construct; an experiment or measurement that answers a definite question.
- Ask when you are about to read through a pile of material, try a set of cases, or make something airtight. Being able to do it in a few minutes is no reason to keep it: done in minutes, it is often done shallowly.
- Do not use it mainly to review your work; that is its weakest use. Give it things to build, find out or measure.

Writing the prompt:
- Say what this part needs, why, and where things stand. Point to the material rather than summarizing it.
- Do not limit which files it may touch or which checks it must pass, and do not tell it how to work: that is where it is strongest.
- State the rules it must keep: what must not break, and the limits of anything shared it will use, such as a server's memory.
- To learn a fact, ask a question facts can answer, not for its opinion of a plan.

When it reports:
- Check the claims you will build on. If it answered a different question, ask again: does it hold, yes or no, and what is the evidence?
- One part, one colleague; several can work at once. Colleagues that might touch the same files each work in their own copy, such as a git worktree, and you merge.";

fn message_colleague_definition() -> Value {
    json!({
        "name": "message_colleague",
        "description": MESSAGE_COLLEAGUE_DESCRIPTION,
        "inputSchema": {
            "type": "object",
            "properties": {
                "to": {
                    "type": "string",
                    "description": "The colleague's id, as ask_colleague or `fastctx w` gave it, such as codex-7f3a. Write codex-7f3a@laptop to name the machine it is on."
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
Send a message to a colleague: one you started, or any other that `fastctx w` shows, including one that is idle or asleep.
- If it is working, the message joins its current turn at its next tool call; otherwise it starts a new turn.
- The last message of the turn it goes into is delivered to you.
- A colleague on a machine that is offline gets it when the machine is back.";

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
