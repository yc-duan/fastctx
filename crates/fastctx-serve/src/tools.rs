//! The World's tools as a session sees them.
//!
//! Everything here is read by a model. It says what the model's working
//! environment is and how to act in it, in words the model already knows:
//! the user's machines, projects, sessions, colleagues, jobs. The design's
//! own words (World, resident, town) stay out.

use fastctx_hands::run::{Content, LONGEST_WAIT, window};
use serde_json::{Value, json};

/// The machines a session works on as its tools were listed: this one, and
/// the others its scope reaches.
pub struct Machines {
    pub here: String,
    pub others: Vec<String>,
    /// Where this session's colleagues live, when the user turned asking
    /// for them on.
    pub colleagues: Option<String>,
}

/// What tells the host to list the tools again.
pub fn list_changed() -> Value {
    json!({ "jsonrpc": "2.0", "method": "notifications/tools/list_changed" })
}

/// The tools this session lists: the shell, and the colleague tools where
/// the user turned asking for colleagues on for this conversation, which a
/// colleague never has: the World is one level deep (`world-colleagues.md`).
/// Writing files and going to other machines are commands in the shell
/// (`world-run.md`, `world-write.md`). `woken` says whether the World can
/// start a turn here when a job ends or an answer comes; elsewhere they come
/// with the answer of the next tool call. `codex` adds what a model calling
/// the tools from Codex's `exec` must set.
pub fn definitions(writes_off: bool, woken: bool, codex: bool, machines: Option<&Machines>) -> Value {
    let mut tools = vec![run_definition(writes_off, woken, codex, machines)];
    if let Some(lives_on) = machines.and_then(|machines| machines.colleagues.as_deref()) {
        tools.push(ask_colleague_definition(woken, lives_on));
        tools.push(message_colleague_definition());
    }
    for tool in &mut tools {
        // Claude Code counts UTF-16 code units here. An answer of BUDGET
        // units holds at most four times as many UTF-8 bytes, and no text
        // takes more UTF-16 units than UTF-8 bytes. Without this, Claude
        // Code 2.1.294 keeps 50,000 inline and saves the rest to a file.
        // Its tool search would otherwise show the model only these tools'
        // names, and `run` stands in for a shell whose description it
        // always saw.
        tool["_meta"] = json!({
            "anthropic/maxResultSizeChars": window::bytes(window::BUDGET),
            "anthropic/alwaysLoad": true,
        });
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

fn run_definition(writes_off: bool, woken: bool, codex: bool, machines: Option<&Machines>) -> Value {
    let writing = if writes_off { "" } else { RUN_WRITING };
    let ends = if woken { RUN_ENDS_WOKEN } else { RUN_ENDS_ASKED };
    let exec = if codex { RUN_EXEC } else { "" };
    json!({
        "name": "run",
        "description": format!(
            "{exec}{} {RUN_START}\n\n{RUN_SHELL}{ends}\n\n{RUN_FILES}{writing}{RUN_COPY}\n\n{RUN_ON}\n\n{}\n\n{RUN_COMMANDS}",
            machine_names(machines),
            run_together(writes_off)
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
                },
                "wait": {
                    "type": "integer",
                    "minimum": 0,
                    "maximum": LONGEST_WAIT.as_secs(),
                    "description": WAIT_DESCRIPTION
                }
            },
            "required": ["command"]
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
- Each call is a fresh login shell, starting in the home directory, or in `cwd`. Nothing carries over between calls, and standard input is empty. Standard output and standard error come back together; output too long to show whole comes back as its start and end, and the answer names the file that holds all of it.
- Calls share nothing, so you can make several at once, and call this tool from your scripts and subagents like any other. The steps of one piece of work belong in one script; separate pieces of work can go in separate calls.
- Images a program draws with the kitty or iTerm2 terminal protocols come back as images.
- A call answers as soon as the script ends. A script still running after 15 seconds, or after `wait` seconds when you set it (up to 600), goes on by itself in the background as a job: it is never stopped for taking long, and the answer names the job and the file its output continues in. `wait` 0 sends the script to the background at once, before it has printed anything: for work you will not wait for.
";

/// Codex's Code Mode hands a nested call's whole answer to the script, but
/// the model sees only what the script prints, cut at the script's
/// `max_output_tokens` (10,000 by default, counted as UTF-8 bytes / 4). Only
/// the script's first-line pragma raises it: no setting or tool definition
/// can. 100,000 holds an answer of `window::BUDGET` units with what a script
/// adds around it; what it prints is recorded under the global tool output
/// limit that apply writes for the same budget (`world-view.md`). It leads
/// the description: a model writing the script reads the tool from its top.
const RUN_EXEC: &str = "\
You must begin every `exec` script that calls this tool with `// @exec: {\"max_output_tokens\": 100000}`, putting any other setting, such as `yield_time_ms`, on that same line: its answers are often long, and a script shows you only what it prints, no more than its `max_output_tokens`.

";

const RUN_ENDS_WOKEN: &str = "\
- When a job of yours ends, you are told automatically, with its exit status and last lines, while you work or after you end your turn. Meanwhile do what suits you: other work, end your turn, or wait for it with `fastctx wait JOB` in a call with a longer `wait`.
- While jobs of yours run, each answer ends with a line naming them.";

const RUN_ENDS_ASKED: &str = "\
- When a job of yours ends, the answer of your next call to this tool says so, with its exit status and last lines; nothing reaches you after you end your turn. Meanwhile do what suits you: other work, or wait for it with `fastctx wait JOB` in a call with a longer `wait`.
- While jobs of yours run, each answer ends with a line naming them.";

const WAIT_DESCRIPTION: &str = "\
The most seconds to wait for the script; the call answers as soon as it ends. 15 by default, at most 600. A script still running then goes on in the background by itself; 0 sends it there at once, before any output.";

const RUN_FILES: &str = "\
Files:
- `fastctx view PATH...` shows files, each as its kind is read: text with line numbers; an image as the image itself, for you to see; a PDF as its text, page by page; Word, PowerPoint and Excel as their content, with Excel cells by address. It shows a lot at once, so give it all the files you need in one call rather than one by one; read the ones the work needs, not everything in sight. When not all of it fits, it says what was shown and what was not. Use it for any file you need to look at:
    fastctx view /srv/app/src/main.rs /srv/app/README.md
    fastctx view --lines 120-180 /srv/app/src/main.rs
    fastctx view C:/Users/me/Pictures/chart.png
    fastctx view --pages 3-5 /srv/app/design.pdf
    fastctx view --image --pages 3 /srv/app/design.pdf
    fastctx view --sheet Sales --range B2:F40 C:/Users/me/q3.xlsx
  `--lines` (text, Word) and `--pages` (PDF pages, PowerPoint slides) apply to every file of the command; for a different range in each file, write one command per file. `--chars 300001-600000` shows those characters of each line, counted as the head lines count them, for a line too long to show at once; `rg -o '.{0,200}WORD.{0,200}'` shows just what is around a match in one. `--image` shows PDF pages as pictures, for layout and figures. Text in any encoding is shown as text, its encoding named in the head line when it is not UTF-8; `--encoding gbk` reads it in an encoding you name, when the one shown is wrong.
- To see how large files or directories are before reading them, use `wc -l`, `ls -lh` or `du -sh` as usual.
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
  The SEARCH text must be in the file exactly once (`--all` replaces every one); all blocks apply, or none. Only the text replaced changes: the file keeps its encoding, BOM and line endings.
- `write PATH` makes standard input the whole file, in the file's encoding and line endings; a new file is UTF-8 (`-n`: no final newline).
- `edit` and `write` take absolute paths. Quote the here-document delimiter, <<'EOF', so bash leaves the text as written.
";

const RUN_COPY: &str = "\
- `fastctx copy SOURCE... DEST` copies files and directories between machines, each end MACHINE:PATH or a path on this machine. DEST is what the copy becomes; end it with / to copy into that directory, as several sources need.";

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

/// The commands as programs a script combines; the example of piping into
/// `write` goes when the World's write commands are off.
fn run_together(writes_off: bool) -> String {
    let write = if writes_off { "" } else { "\n    python3 render.py | write /srv/app/config.json" };
    format!(
        "\
Putting commands together: every command here, `on` and the fastctx ones included, is an ordinary program in your script. Its arguments, standard input and output, and exit code work as any command's, so combine them as you would `cat` or `grep`: pipelines, loops, `$( )`, `&&`, `xargs`, `&` with `wait`. For example:
    fastctx view /srv/app/src/*.rs
    rg -l0 'TODO' /srv/app | fastctx view --files0-from -{write}
    for m in laptop server; do on \"$m\" df -h / & done; wait
    diff <(on laptop cat C:/work/app.ini) <(on server cat /srv/app/app.ini)"
    )
}

const RUN_COMMANDS: &str = "\
More commands (`--help` on each gives all its options):
- `fastctx nodes` lists the machines, which are online, and what each runs.
- `fastctx w` shows what runs and who works now, and what happened since your last turn; `fastctx w ID...` shows those sessions or jobs.
- `fastctx wait JOB...` waits until jobs end and shows how each ended; `fastctx stop JOB...` stops them and everything they started.
- `fastctx project` lists the user's projects and shows their maps. A fastctx project is not a folder or a repository but a named piece of work that can span several machines; its map says what it is for and where its code and data are on each. `fastctx project enter NAME` makes you work in one; enter one only when the user tells you which project to work on.
- `fastctx journal` reads and adds to the project's journals, the history of its work. `fastctx page` publishes an HTML page the user opens in the World app.";

fn ask_colleague_definition(woken: bool, lives_on: &str) -> Value {
    let delivered = if woken { "" } else { " with the answer of your next call to a fastctx tool" };
    json!({
        "name": "ask_colleague",
        "description": ASK_COLLEAGUE_DESCRIPTION.replace("{delivered}", delivered).replace("{lives_on}", lives_on),
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
                }
            },
            "required": ["prompt", "description"]
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

A colleague is not a subagent inside this conversation. It lives on {lives_on}, the machine the user chose for this conversation's colleagues, and its scripts start there; it reaches the other machines the same way you do, with `on`. It knows nothing of this conversation, so the prompt must say which machines and paths it works on. It keeps working if you stop, and `fastctx w ID` shows what it is doing. Each time it ends a turn, its last message is delivered to you{delivered}, under a line saying how the turn ended: that message is its answer, or its question to you. Reply, or give it more, with message_colleague.

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
            "required": ["to", "message"]
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
