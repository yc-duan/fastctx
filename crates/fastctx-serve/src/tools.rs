//! The World's tools as a session sees them.
//!
//! Everything here is read by a model. It says what the model's working
//! environment is and how to act in it, in words the model already knows:
//! the user's machines, projects, sessions, colleagues, jobs. The design's
//! own words (World, resident, town) stay out.

use fastctx_hands::run::{Content, LONGEST_WAIT, window};
use serde_json::{Value, json};

/// The machines a session may run its scripts on, as its tools were listed:
/// the one it lives on where the accounts it works for may work there, and
/// the others they reach.
pub struct Machines {
    /// The machine the session lives on, where it may run there.
    pub here: Option<String>,
    /// The other machines it may work on.
    pub others: Vec<String>,
    /// Those of them their owners put in groups.
    pub groups: Vec<fastctx_types::world::MachineGroup>,
    pub collab: Option<fastctx_types::collab::Offered>,
    pub scratch: fastctx_types::scratch::Descriptions,
    /// Whether the session speaks to the user through `message_user`.
    pub speaks: bool,
}

impl Machines {
    /// Every machine it may run on, by name.
    fn all(&self) -> Vec<String> {
        self.here.iter().cloned().chain(self.others.iter().cloned()).collect()
    }

    /// The machines as the description lists them, group by group.
    fn listed(&self) -> String {
        let describe = |name: &String| {
            self.scratch
                .get(name)
                .map(|path| fastctx_types::scratch::words::described(name, path))
                .unwrap_or_else(|| name.clone())
        };
        let names = self.all().iter().map(describe).collect::<Vec<_>>();
        let mut groups = self.groups.clone();
        for group in &mut groups {
            group.machines = group.machines.iter().map(describe).collect();
        }
        fastctx_types::world::listed(&names, &groups, None)
    }
}

/// What tells the host to list the tools again.
pub fn list_changed() -> Value {
    json!({ "jsonrpc": "2.0", "method": "notifications/tools/list_changed" })
}

/// The tools this session lists: the shell, and the colleague tools where
/// an account enabled for this caller, which a
/// colleague never has: the World is one level deep (`world-colleagues.md`).
/// Writing files and going to other machines are commands in the shell
/// (`world-run.md`, `world-write.md`). `woken` says whether the World can
/// start a turn here when a job ends or an answer comes; elsewhere they come
/// with the answer of the next tool call. `codex` adds what a model calling
/// the tools from Codex's `exec` must set.
pub fn definitions(writes_off: bool, woken: bool, codex: bool, machines: Option<&Machines>) -> Value {
    let mut tools = vec![run_definition(writes_off, woken, codex, machines)];
    if let Some(collab) = machines.and_then(|machines| machines.collab.as_ref()) {
        tools.push(ask_colleague_definition(collab));
        tools.push(message_colleague_definition());
    }
    if machines.is_some_and(|machines| machines.speaks) {
        tools.push(message_user_definition());
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

/// Which machines the session may run on and how a script names one, first
/// thing in the description, said the same way for every session; a model
/// that knows them reaches the right one by the user's words. Where the
/// session itself lives is not said (`world-run.md`, 脚本在哪台机器上跑).
fn opening(machines: Option<&Machines>) -> String {
    const NAMED: &str = "Write each script as one `on MACHINE`: `on laptop cargo test` for a command, or for a script\n    on laptop <<'RUN'\n    cd ~/app && cargo test\n    RUN\nIt runs there in a fresh login shell, from that machine's home; inside it, `on` runs parts of it on the other machines. A script that is not one `on` is refused before anything of it runs. This tool is your shell on all of them, with no address, login or key; `fastctx nodes` shows which are online.";
    let Some(machines) = machines else {
        return format!(
            "This tool runs a bash script on one of the user's machines; `fastctx nodes` lists them. {NAMED}"
        );
    };
    if machines.all().is_empty() {
        return "This tool runs a bash script on one of the user's machines, and the user has none connected to fastctx yet: nothing can run. When the work needs a shell, tell the user to connect a machine of theirs.".to_string();
    }
    format!("This tool runs a bash script on one of these machines: {}. {NAMED}", machines.listed())
}

fn run_definition(writes_off: bool, woken: bool, codex: bool, machines: Option<&Machines>) -> Value {
    let writing = if writes_off { "" } else { RUN_WRITING };
    let ends = if woken { RUN_ENDS_WOKEN } else { RUN_ENDS_ASKED };
    let exec = if codex { RUN_EXEC } else { "" };
    let paths = run_paths(writes_off);
    let properties = json!({
        "command": {
            "type": "string",
            "description": "The bash script to run: one `on MACHINE`, with the work inside it."
        },
        "wait": {
            "type": "integer",
            "minimum": 0,
            "maximum": LONGEST_WAIT.as_secs(),
            "description": WAIT_DESCRIPTION
        }
    });
    json!({
        "name": "run",
        "description": format!(
            "{exec}{}\n\n{RUN_SHELL}{RUN_START}\n{RUN_SHELL_REST}{ends}\n\nFiles:\n{paths}{RUN_FILES}{writing}{RUN_COPY}\n\n{}\n\n{}\n\n{RUN_COMMANDS}",
            opening(machines),
            run_on(writes_off),
            run_together(writes_off),
        ),
        "inputSchema": {
            "type": "object",
            "properties": properties,
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

const RUN_SHELL: &str = "\
Shell:
- Every machine runs bash with the GNU tools and `rg`, Windows ones too (Git Bash). Write Windows paths like C:/Users/me, and use `ls`, `find`, `du` and `df` rather than PowerShell.
- Your script reaches bash as a file, exactly as you wrote it: no other shell parses it on the way, so nothing in it is unquoted, halved or cut. A program can run inline in it, in a here-document with a quoted delimiter, as written.
- Git Bash rewrites arguments that look like Unix paths, such as /FI, before a Windows program sees them. Put MSYS_NO_PATHCONV=1 before such a command, and before `cmd /c`, which otherwise silently does nothing.
";

/// How each call starts.
const RUN_START: &str = "- Each call is a fresh login shell on the machine it names, starting in that machine's home. Nothing carries over between calls, and standard input is empty. Standard output and standard error come back together; output too long to show whole comes back as its start and end, and the answer names the file that holds all of it.";

const RUN_SHELL_REST: &str = "\
- Keep temporary files in the scratch path given for that machine, not in /tmp or the project. Paths not listed above come with the first reply from that machine. They survive runs and are deleted with this conversation.
- Calls share nothing, so you can make several at once, and call this tool from your scripts and subagents like any other. The steps of one piece of work belong in one script; separate pieces of work can go in separate calls.
- Images a program draws with the kitty or iTerm2 terminal protocols come back as images.
- A call answers as soon as the script ends. A script still running after 15 seconds, or after `wait` seconds when you set it (up to 600), goes on by itself in the background as a job: it is never stopped for taking long, and the answer names the job and the file its output continues in. `wait` 0 sends the script to the background at once, before it has printed anything: for work you will not wait for.
";

/// Codex's Code Mode hands a nested call's whole answer to the script, but
/// the model sees only what the script prints, cut at the script's
/// `max_output_tokens` (10,000 by default, counted as UTF-8 bytes / 4). Only
/// the script's first-line pragma raises it: no setting or tool definition
/// can. 60,000 holds an answer of `window::BUDGET` units with what a script
/// adds around it; what it prints is recorded under the global tool output
/// limit that apply writes for the same budget (`world-view.md`). It leads
/// the description: a model writing the script reads the tool from its top.
const RUN_EXEC: &str = "\
You must begin every `exec` script that calls this tool with `// @exec: {\"max_output_tokens\": 60000}`, putting any other setting, such as `yield_time_ms`, on that same line: its answers are often long, and a script shows you only what it prints, no more than its `max_output_tokens`.

";

const RUN_ENDS_WOKEN: &str = "\
- When a job of yours ends, you are told automatically, with its exit status and last lines, while you work or after you end your turn, unless a `fastctx wait` of yours already took that end. Meanwhile do what suits you: other work, end your turn, or wait for it with `fastctx wait JOB` in a call with a longer `wait`.
- While jobs of yours run, each answer ends with a line naming them.";

const RUN_ENDS_ASKED: &str = "\
- When a job of yours ends, the answer of your next call to this tool says so, with its exit status and last lines, unless a `fastctx wait` of yours already took that end; nothing reaches you after you end your turn. Meanwhile do what suits you: other work, or wait for it with `fastctx wait JOB` in a call with a longer `wait`.
- While jobs of yours run, each answer ends with a line naming them.";

const WAIT_DESCRIPTION: &str = "\
The most seconds to wait for the script; the call answers as soon as it ends. 15 by default, at most 600. A script still running then goes on in the background by itself; 0 sends it there at once, before any output.";

/// The rule every command for files keeps, said once for all of them that
/// are on (`world-write.md`, `world-view.md`).
fn run_paths(writes_off: bool) -> &'static str {
    if writes_off {
        "- `fastctx view` and `fastctx copy` take only absolute paths, of the machine they run on: /srv/app/main.rs, or C:/Users/me/app/main.rs on Windows. A relative path is refused, and nothing is read or written: the directory a script is in is often not the one you think.\n"
    } else {
        "- `fastctx view`, `edit`, `write` and `fastctx copy` take only absolute paths, of the machine they run on: /srv/app/main.rs, or C:/Users/me/app/main.rs on Windows. A relative path is refused, and nothing is read or written: the directory a script is in is often not the one you think.\n"
    }
}

const RUN_FILES: &str = "\
- `fastctx view PATH...` shows files, each as its kind is read: text with line numbers; an image as the image itself, for you to see; a PDF as its text, page by page; Word, PowerPoint and Excel as their content, with Excel cells by address. Without a range, a file too big to show whole comes back as its first lines and an outline of where its headings and definitions are (a PDF's bookmarks, a deck's slide titles, a workbook's sheets), or, with no outline to give, as its beginning and its end; several files looked at together share what one file would get, the small ones shown whole. With a range (`--lines`, `--chars`, `--pages`, `--sheet`), that range is shown in full. When you are unsure what a file holds, look at it without a range first. It shows a lot at once, so give it all the files you need in one call rather than one by one; read the ones the work needs, not everything in sight. When not all of it fits, it says what was shown and what was not. Use it for any file you need to look at:
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
  When the file or your text has lines like these markers, write the block's markers longer than those lines, all three the same length, such as <<<<<<<<< SEARCH, ========= and >>>>>>>>> REPLACE.
- `write PATH` makes standard input the whole file, in the file's encoding and line endings; a new file is UTF-8 (`-n`: no final newline).
- Quote the delimiter of a here-document for `edit` or `write`, <<'EOF', so bash leaves the text as written.
";

const RUN_COPY: &str = "\
- `fastctx copy SOURCE... DEST` copies files and directories, on one machine or between machines, each end MACHINE:PATH or a path on the machine it runs on. DEST is what the copy becomes; end it with / to copy into that directory, as several sources need.";

/// `on` inside a script, which runs a command or a part of it on another
/// machine; with the World's write commands off, without the example that
/// writes there.
fn run_on(writes_off: bool) -> String {
    let write = if writes_off {
        ""
    } else {
        "\n    on server write /srv/app/config.json <<'EOF'\n    {\"port\": 8081}\n    EOF"
    };
    format!(
        "\
Other machines, from inside a script: write `on MACHINE` before a command to run it there. As with ssh, standard input is the command's when a command follows, and the script to run there when none does:
    on laptop cargo test --release{write}
    on server <<'EOF'
    cd /srv/app && tail -n 200 app.log | grep ERROR
    EOF
- The part sent there sees your script's variables, functions and shell options; PATH, HOME, LANG and the machine's other settings are its own.
- Its output and exit code work like any command's, in pipelines and `if` too.
- A script quoted as a single word is refused: give it as a here-document, with a delimiter other than the one around the whole script.
- If the connection drops, it goes on there, and its end still reaches you.
- Every command works on every machine, with that machine's paths, such as `on server fastctx view /srv/app/app.ini`."
    )
}

/// The commands as programs a script combines; the example of piping into
/// `write` goes when the World's write commands are off.
fn run_together(writes_off: bool) -> String {
    let write = if writes_off { "" } else { "\n    python3 render.py | write /srv/app/config.json" };
    format!(
        "\
Putting commands together: every command, `on` and the fastctx ones included, is an ordinary program in your script. Its arguments, standard input and output, and exit code work as any command's, so combine them as you would `cat` or `grep`: pipelines, loops, `$( )`, `&&`, `xargs`, `&` with `wait`. For example, inside one `on laptop <<'RUN'`:
    fastctx view /srv/app/src/*.rs
    rg -l0 'TODO' /srv/app | fastctx view --files0-from -{write}
    for m in laptop server; do on \"$m\" df -h / & done; wait
    diff <(cat C:/work/app.ini) <(on server cat /srv/app/app.ini)"
    )
}

const RUN_COMMANDS: &str = "\
More commands (`--help` on each gives all its options):
- `fastctx nodes` lists the machines, which are online, and what each runs.
- `fastctx w` shows what runs and who works now, and what happened since your last turn; `fastctx w ID...` shows those sessions or jobs.
- `fastctx wait JOB...` waits until jobs end and shows how each ended; `fastctx stop ID...` stops jobs and their processes, or a colleague's current turn.
- `fastctx project` lists the user's projects and shows their records. A fastctx project is not a folder or a repository but a named piece of work that can span several machines; its record says what it is for and where its code and data are on each. `fastctx project enter NAME` makes you work in one; enter one only when the user tells you which project to work on.
- `fastctx page` publishes an HTML page the user opens in the World app.
- `fastctx plan` shows this conversation's plan; `fastctx plan --file DRAFT` puts up its next version.
- `fastctx sense` leaves a sentinel to wait for something and wake you when it happens.";

fn ask_colleague_definition(collab: &fastctx_types::collab::Offered) -> Value {
    let lives = collab.callees.iter().map(|callee| {
        let lives = match &callee.lives_on {
            Some(lives_on) => format!("It lives on {lives_on}, the machine configured for this callee; it runs its scripts on the machines it may use, each one `on MACHINE`, as you do."),
            None => "It is another account's agent assisting the user: it runs its scripts on the user's machines, each one `on MACHINE`, as you do.".to_string(),
        };
        let account = callee.account.as_ref().map(|account| format!("It works for account {account}. ")).unwrap_or_default();
        format!("{}: {}\n{account}{lives}", callee.name, callee.use_when)
    }).collect::<Vec<_>>().join("\n\n");
    let mut tool = json!({
        "name": "ask_colleague",
        "description": format!("{}\n\n{lives}\n\n{}", fastctx_types::collab_prompt::ASK_DESCRIPTION, collab.prompt),
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
    });
    if collab.callees.len() > 1 {
        let names = collab.callees.iter().map(|callee| callee.name.clone()).collect::<Vec<_>>();
        tool["inputSchema"]["properties"]["callee"] = json!({ "type": "string", "enum": names,
            "description": "The callee's offered name, including its account when qualified." });
        tool["inputSchema"]["required"].as_array_mut().expect("required fields").push("callee".into());
    }
    tool
}

fn message_colleague_definition() -> Value {
    json!({
        "name": "message_colleague",
        "description": fastctx_types::collab_prompt::MESSAGE_DESCRIPTION,
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

fn message_user_definition() -> Value {
    json!({
        "name": fastctx_types::voice::TOOL,
        "description": fastctx_types::voice::DESCRIPTION,
        "inputSchema": {
            "type": "object",
            "properties": {
                "message": {
                    "type": "string",
                    "description": "What the user reads, in Markdown."
                },
                "status": {
                    "type": "string",
                    "enum": ["reply", "info", "urgent"],
                    "description": "reply answers what the user just said; info and urgent are yours to start, urgent when they should know soon."
                },
                "attachments": {
                    "type": "array",
                    "maxItems": fastctx_types::voice::ATTACHMENTS_MOST,
                    "description": "Files shown with the message.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "machine": { "type": "string", "description": "The machine the file is on, as `on` names it." },
                            "path": { "type": "string", "description": "Its path there." }
                        },
                        "required": ["machine", "path"]
                    }
                }
            },
            "required": ["message", "status"]
        },
        "annotations": {
            "title": "Message the user",
            "readOnlyHint": false,
            "destructiveHint": false,
            "idempotentHint": false,
            "openWorldHint": false
        }
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
