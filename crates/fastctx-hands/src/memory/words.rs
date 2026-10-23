//! Model-facing memory text, kept together for editorial changes.

pub const INTRO: &str = "PROJECT MEMORY\n\
This project keeps journals and specs as Markdown files, each starting with\n\
---\n\
name: <the file's name>\n\
description: <one line: what it covers>\n\
---\n";
pub const READ: &str = "Read them with `fastctx view` or `rg` when your work touches them.\n\n";
pub const READ_ONLY: &str = "Project journals, specs and attributes are read-only for colleagues. Give findings and evidence paths to the conversation that asked you.\n";
pub const UNAVAILABLE: &str = "Memory file locations are unavailable:";
pub const WRITE: &str = "\
Journals are records. Write one when whoever works here next needs something they cannot
recover from the specs, the code or git: you tried something and it did not go as you meant,
or you turned to another path; something you were finding out came out one way or another;
you are stopping, handing off or stuck, and where the work stands. Routine work is not a
record. Write sentences that will still be true a year from now: what you did, what you meant
to find out, what you saw, under which conditions (versions, machine, inputs), with dates,
never \"now\" or \"recently\". Quote the user verbatim; cite code as path@commit. Name a journal
after a thing, not a conclusion. Look for earlier journals on the same thing first; when
yours answers or corrects one, name it. Create a journal with `write` in the journals folder;
it is never changed afterwards: to correct one, write a new one that names it. Files kept as
evidence go in a folder of the same name.

Specs hold only what the user has confirmed: one file per thing, the current understanding,
told plainly. When a conclusion changes, edit the paragraphs it touches with `edit`; keep
what is still open under its own heading.
";

pub fn older(count: usize) -> String {
    format!("{count} older journals are in that folder.\n")
}

pub fn changed(by: &str, revision: &fastctx_types::memory::Revision) -> String {
    let action = match revision.kind {
        fastctx_types::memory::Kind::Journal => "wrote journal",
        fastctx_types::memory::Kind::Spec => "saved spec",
    };
    format!("{by} {action} {} (version {}).", revision.name, revision.version)
}
