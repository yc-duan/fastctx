//! What a session the user opens is told about its working environment, in
//! its host's guidance file (`world-run.md`, 模型怎样知道): the user's
//! machines, that `run` is its shell on all of them and its way to look at
//! and change files on this machine too, since the host's own shell and image
//! viewer are off; and the one rule a session needs before it touches
//! anything, that it enters a project only when the user says.
//!
//! A host may list a server's tools by name only until they are used, so
//! these words alone must lead a model to the tools and give the commands it
//! reaches for most; `run`'s description gives them in full.

/// What a Codex session must do before anything else works at full size,
/// set apart at the end of the block, where a model meets it last.
/// Codex's Code Mode hands a nested call's whole answer to the `exec`
/// script, but the model sees only what the script prints, cut at the
/// script's `max_output_tokens` (10,000 by default); only the script's
/// first-line pragma raises it, no setting can. A rule in the run tool's
/// description alone was not followed (0 of 3 with gpt-6-luna at low
/// effort, 2026-10-09), nor one that applied only to scripts calling `run`
/// (0 of 4); this unconditional one was (4 of 4). 100,000 holds a whole
/// answer of the World's budget with what a script adds around it.
const CODEX_EXEC: &str = "
## fastctx in Codex

You must begin every `exec` script with this exact first line, every time:

    // @exec: {\"max_output_tokens\": 100000}

fastctx answers are long, and without that line you see only their first part. Put any other setting, such as `yield_time_ms`, into that same line.
";

/// The block of guidance for a host's sessions; `codex` adds what only a
/// Codex session needs.
pub fn guidance(codex: bool) -> String {
    let exec = if codex { CODEX_EXEC } else { "" };
    format!(
        "\
## Your machines and tools

The user has several machines, connected by fastctx: this one, and others that `fastctx nodes` lists. You work on all of them through the `run` tool.

`run` is your shell: bash with the GNU tools and `rg` on every machine, Windows included. Do all shell work with it, and all work on files, on this machine too:
- `fastctx view PATH...` shows files: text with line numbers (`--lines 120-180` for part of one); an image as the image itself, for you to see; a PDF as its text by page (`--pages 3-5`, and `--image` to see the pages); Word, PowerPoint and Excel as their content.
- `edit PATH` and `write PATH` change a file and keep its encoding and line endings.
- `rg` searches.

`run`'s description shows each of these in full.

`run` starts on this machine. To work on another one, write `on MACHINE` before a command, or give it a script:

```bash
on server tail -n 50 /var/log/app.log
on laptop <<'EOF'
cd C:/work/app && cargo test
EOF
```

When the user speaks of one of the machines, work on it this way.

Inside a script, these commands are ordinary programs: give them several files, pipe into and out of them, loop over machines, as with any command. Calls to `run` share nothing: make several at once, or call `run` from your scripts and subagents, as with any tool.

fastctx also keeps the user's ongoing work as projects. A fastctx project is not a folder or a repository: it is a named piece of work that can span several machines. Its record says what it is for and where its code and data are on each machine; its journals hold the history of the work. `fastctx project list` lists them. Enter one with `fastctx project enter NAME` only when the user tells you which project to work on; entering gives you its map.

`ask_colleague` starts a GPT colleague that takes a part of the work and does it on its own while you go on.
{exec}"
    )
}
