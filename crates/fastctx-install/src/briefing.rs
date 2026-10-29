//! What a session the user opens is told about its working environment, in
//! its host's guidance file (`world-run.md`, 模型怎样知道): that `run` is its
//! shell, on this machine too, since the host's own shell is off, and works
//! on files on every machine it reaches; and that each script is one
//! `on MACHINE` naming a machine `run`'s description lists. Nothing else of
//! the World is said there: the user's own sessions are given `run` and
//! nothing more (`world-layers.md`), and the World's own sessions are told the
//! rest in their own instructions. It is written once, when fastctx is
//! applied, and many users have only this one computer, so it never says the
//! user has several machines: what is true at the moment of a call is in
//! `run`'s description, written when the session lists its tools.
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
/// (0 of 4); this unconditional one was (4 of 4). 60,000 holds a whole
/// answer of the World's budget with what a script adds around it.
pub const CODEX_EXEC: &str = "
## fastctx in Codex

You must begin every `exec` script with this exact first line, every time:

    // @exec: {\"max_output_tokens\": 60000}

fastctx answers are long, and without that line you see only their first part. Put any other setting, such as `yield_time_ms`, into that same line.
";

/// The block of guidance for a host's sessions; `codex` adds what only a
/// Codex session needs.
pub fn guidance(codex: bool) -> String {
    let exec = if codex { CODEX_EXEC } else { "" };
    format!(
        "\
## Your shell and tools

You work through fastctx's `run` tool. `run` is your shell: bash with the GNU tools and `rg`, Windows included. Do all shell work with it, on this machine too. On every machine it reaches, it also works on files:
- `fastctx view PATH...` shows files: text with line numbers (`--lines 120-180` for part of one); an image as the image itself, for you to see; a PDF as its text by page (`--pages 3-5`, and `--image` to see the pages); Word, PowerPoint and Excel as their content. Without a range, a large file comes back as its first lines and an outline, or its beginning and its end, and several files together get what one would; with a range, that range in full. When you are unsure what a file holds, look at it without a range first.
- `edit PATH` and `write PATH` change a file and keep its encoding and line endings.
- `rg` searches.

`run`'s description shows each of these in full.

`run` runs each script on one of the machines its description lists, this computer among them, the whole script written as one `on MACHINE`; its description says how. When the user speaks of one of those machines, work on it.

Inside a script, these commands are ordinary programs: give them several files, pipe into and out of them, loop over machines, as with any command. Calls to `run` share nothing: make several at once, or call `run` from your scripts and subagents, as with any tool.
{exec}"
    )
}
