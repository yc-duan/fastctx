//! What a session the user opens is told about the World, in its host's
//! guidance file (`world-run.md`, 模型怎样知道): which tools are the World's,
//! the commands its shell has, and the one rule a session needs before it
//! touches anything, that it enters a project only when the user says.
//!
//! The words are the model's: the World's commands under their names, a
//! town as the commands call a project.

/// The writing tools a host's sessions are given.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Writer {
    /// `edit` and `write`, as Claude Code's own.
    Edit,
    /// `apply_patch`, as Codex's own.
    ApplyPatch,
}

/// The block of guidance for a host whose sessions write with `writer`.
pub fn guidance(writer: Writer) -> String {
    let write = match writer {
        Writer::Edit => "`edit` and `write` change files",
        Writer::ApplyPatch => "`apply_patch` changes files",
    };
    format!(
        "\
## The World

This machine belongs to a World: the user's machines, and the agents working on them, connected by fastctx. Its tools are in this session: `run` runs a bash script, {write}, each on any machine of the World; `ask_colleague` brings in a GPT colleague that works on its own while you go on.

When you work in a town, the World's name for a project, run commands and change files with these tools rather than your host's own: the work then happens on the machine it belongs to, the others in the town can see it, and what you wrote is known to be yours.

In `run`, beside each machine's own programs:
- `fastctx view` shows files, several at once, with line numbers, and images as images.
- `fastctx w` shows who is in your town and what each is doing.
- `fastctx project enter <town>` moves you into a town. Use it only when the user tells you which project to work on.
- `fastctx journal` reads and adds to your town's journals, the history of its work.
- `fastctx page` publishes a page into your town: HTML the user opens in World's interface, live with the World's facts. `fastctx page --help` says how to write one.

`fastctx --help` lists every command; each explains itself with its own `--help`.
"
    )
}
