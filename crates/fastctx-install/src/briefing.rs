//! What a session the user opens is told about the World, in its host's
//! guidance file (`world-run.md`, 模型怎样知道): that the World's other
//! machines are at hand by name through its tools, and the one rule a
//! session needs before it touches anything, that it enters a project only
//! when the user says.
//!
//! A host may list a server's tools by name only until they are used, so
//! these words alone must lead a model to the tools. What the shell's
//! commands do is said once, in `run`'s own description.

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
        Writer::Edit => "`edit` and `write` change its files",
        Writer::ApplyPatch => "`apply_patch` changes its files",
    };
    format!(
        "\
## The World

This machine belongs to a World: the user's machines, connected by fastctx, and the agents working on them. Each of those machines is at hand from this session by its name, with no address, login or key: `run` runs a bash script on it and {write}, given its name as `node`; without `node` they act here. `fastctx nodes`, in `run`, lists the machines and what each runs. When the user speaks of one of them, work on it this way.

When you work in a town, the World's name for a project, run commands and change files with these tools rather than your host's own: the work then happens on the machine it belongs to, the others in the town can see it, and what you wrote is known to be yours. Enter a town with `fastctx project enter <town>` only when the user tells you which project to work on.

`ask_colleague` brings in a GPT colleague that works on its own while you go on. `run`'s description lists the World's other commands, and `fastctx --help` explains each.
"
    )
}
