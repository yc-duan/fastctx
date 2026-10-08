//! What a session the user opens is told about the World, in its host's
//! guidance file (`world-run.md`, 模型怎样知道): that `run` is its shell,
//! the way to look at and change files on this machine too, since the host's
//! own shell and image viewer are off; that the World's other machines are
//! at hand by name through it; and the one rule a session needs before it
//! touches anything, that it enters a project only when the user says.
//!
//! A host may list a server's tools by name only until they are used, so
//! these words alone must lead a model to the tools. What the shell's
//! commands do is said once, in `run`'s own description.

/// The block of guidance for a host's sessions.
pub fn guidance() -> String {
    "\
## The World

`run` is this session's shell: bash on this machine, and through `on` on the user's other machines. Do everything a shell is for with it, here as well as there, looking at files included: `fastctx view PATH` shows text, images, PDFs, and Word, PowerPoint and Excel documents; `rg` searches; `edit` and `write` change files, keeping their encoding and line endings.

This machine belongs to a World: the user's machines, connected by fastctx, and the agents working on them. Each of them is at hand by its name, with no address, login or key: in `run`, `on MACHINE` followed by a command, or by a script as a here-document, runs it there; `fastctx nodes` lists the machines. When the user speaks of one of them, work on it this way.

Enter a town, the World's name for a project, with `fastctx project enter TOWN` only when the user tells you which project to work on. `ask_colleague` brings in a GPT colleague that works on its own while you go on. `run`'s description lists the World's commands, and each explains itself with `--help`.
"
    .to_string()
}
