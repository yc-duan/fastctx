//! Project-memory guidance, replies and refusal text.

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

pub fn changed(by: &str, revision: &crate::memory::Revision) -> String {
    let action = match revision.kind {
        crate::memory::Kind::Journal => "wrote journal",
        crate::memory::Kind::Spec => "saved spec",
    };
    format!("{by} {action} {} (version {}).", revision.name, revision.version)
}

pub const NAME_LENGTH: &str = "a memory name must contain 1 to 240 UTF-8 bytes";
pub const NAME_CHARACTERS: &str = "a memory name uses letters, numbers and hyphens; combining marks follow a letter";
pub const RESERVED_NAME: &str = "this memory name is reserved by Windows";
pub const HEADER_START: &str = "memory Markdown must start with ---";
pub const HEADER_NAME: &str = "the header's name must match the Markdown filename";
pub const HEADER_DESCRIPTION: &str = "the header needs a nonempty one-line description";
pub const HEADER_FIELD: &str = "each header field must be key: value on one line";
pub const HEADER_END: &str = "the memory header needs a closing ---";
pub const NAME_TAKEN: &str = "a memory name already belongs to a different document";
pub const IMMUTABLE_JOURNAL: &str = "journals cannot be changed; write another journal that names the correction";
pub const EVIDENCE_JOURNAL: &str = "evidence must belong to an existing journal";
pub const IMMUTABLE_EVIDENCE: &str = "journal evidence already exists and cannot be changed";
pub const MISSING_DOCUMENT_TEXT: &str = "the document's Markdown is missing from its ledger";
pub const APPEND_SHAPE: &str = "a memory append contains one document revision or evidence file";
pub const MISSING_METADATA: &str = "the memory append has no metadata";
pub const EVIDENCE_DIGEST: &str = "evidence needs a lowercase SHA-256 digest";
pub const EVIDENCE_PATH: &str = "the evidence metadata path does not match its name";
pub const EXTERNAL_EVIDENCE: &str = "external evidence needs a location and contains no inline bytes";
pub const MISSING_EVIDENCE_BYTES: &str = "evidence bytes are missing";
pub const EVIDENCE_MISMATCH: &str = "the evidence bytes do not match their metadata";
pub const REVISION_PATH: &str = "the memory revision paths or version are invalid";
pub const MISSING_MARKDOWN: &str = "the memory Markdown is missing";
pub const DOCUMENT_TOO_LARGE: &str = "the memory document exceeds 8 MiB";
pub const HEADER_ATTRIBUTION: &str = "the memory header does not match its World attribution";
pub const VERSION_EXHAUSTED: &str = "the memory version is exhausted";
pub const INACTIVE_JOB: &str = "memory write does not belong to an active job";
pub const MISSING_JOURNAL: &str = "evidence needs an existing journal";
pub const EVIDENCE_WRITE: &str = "journal evidence can only be created with write";
pub const OTHER_DOCUMENT: &str = "this name belongs to a different memory document";
pub const MISSING_SIGNED_TEXT: &str = "the signed memory text is missing";
pub const UTF8_REQUIRED: &str = "memory Markdown must be UTF-8";
pub const STALE_SPEC: &str = "this spec changed since you viewed it; read its latest version before saving";
pub const COLLEAGUE_READ_ONLY: &str =
    "colleagues read project memory; give findings to the conversation that asked you";
pub const WRITER_GONE: &str = "the writing conversation no longer exists";
pub const WRITER_UNANSWERED: &str = "the conversation's machine did not identify its writer";
pub const WRITER_INVISIBLE: &str = "the remote writing conversation is not visible";
pub const PRIMARY_WRITER: &str = "only a user or a primary conversation writes project memory";
pub const HUB_GONE: &str = "the project's hub is no longer connected";
pub const OWN_COMPUTER_PATHS: &str = "memory paths must come from the account's own computer";
pub const REPLICA_NOT_READY: &str = "this computer has not synchronized the project's replica yet";
pub const MISSING_IDENTITY: &str = "the project has no ledger identity";
pub const MISSING_ACCOUNT: &str = "the project has no account";
pub const COMPUTER_UNANSWERED: &str = "the computer did not return its capabilities";
pub const MISSING_NETWORK_NAME: &str = "the project has no network name";
pub const PATHS_UNANSWERED: &str = "the computer did not return memory paths";
pub const PATH_ACCESS_ENDED: &str = "access to the memory computer ended while its paths were read";
pub const OFFLINE_COMPUTERS: &str =
    "none of this account's own computers are online; journal and spec bodies cannot be opened or written";
pub const UNKNOWN_CONVERSATION: &str = "unknown conversation";
pub const MISSING_PROJECT_RECORD: &str = "the project record is unavailable";
pub const PROJECT_UNLISTED: &str = "the hub no longer lists the project";
pub const NO_PROJECT_WRITER: &str = "no caller account may write this project";
pub const REDIRECTED_PATH: &str = "a project-memory path was redirected outside its replica";
pub const MARKDOWN_EXTENSION: &str = "memory documents have the .md extension";
pub const WRITE_TARGET: &str = "write a Markdown document or a file in a journal's evidence folder";
pub const EVIDENCE_FOLDER: &str = "evidence must stay inside its journal folder";
pub const SETTING_UNCONFIRMED: &str = "the hub did not confirm the memory setting";
pub const HUB_TOO_OLD: &str = "this hub is too old for project memory; update it";
pub const INDEX_UNANSWERED: &str = "the hub answered something other than the project index";
pub const DOCUMENT_UNANSWERED: &str = "the hub answered something other than a memory document";
pub const MISSING_SIGNED_REVISION: &str = "the hub returned no signed memory revision";
pub const REVISION_MISMATCH: &str = "the signed memory revision does not match the request or history";
pub const MISSING_SIGNED_MARKDOWN: &str = "the signed Markdown is missing";
pub const APPEND_UNCONFIRMED: &str = "the hub answered something other than a committed memory append";
pub const BODY_DEVICE_REQUIRED: &str =
    "project bodies are read on the account's own devices; an assisting host receives only the index";
pub const NO_LEDGER: &str = "the project has no ledger yet";
pub const NO_DOCUMENT: &str = "no such memory document or revision";
pub const MISSING_REVISION_ENTRY: &str = "the memory revision has no ledger entry";
pub const MISSING_OPENING: &str = "the project's opening record is missing";
pub const BUSY_READING: &str = "the hub is busy reading project memory";
pub const READ_DEADLINE: &str = "reading project memory exceeded 55 seconds";
pub const MISSING_RECORD: &str = "the project record is missing";
pub const INDEX_ACCESS_ENDED: &str = "project access ended while its index was read";
pub const LEGACY: &str = "This journal interface was retired; use project memory Markdown files.";
pub const CATALOG_UNANSWERED: &str = "the World did not return a memory catalog";
pub const WORLD_DOCUMENT_UNANSWERED: &str = "the World did not return a memory document";
pub const SAVE_UNCONFIRMED: &str = "the World did not confirm the memory revision";

pub const SHOW_HELP: &str = "A project's attributes and enabled memory index.";

pub const HEADER_QUOTE: &str = "a quoted header field needs its closing quote";
pub const RETAINED_BYTES: &str = "retained evidence has different bytes";
pub const REVISION_NO_MARKDOWN: &str = "a memory revision has no Markdown";
pub const EVIDENCE_MISSING: &str = "journal evidence is missing";
pub const EVIDENCE_NOT_FILE: &str = "the retained evidence is not a regular file";
pub const EVIDENCE_CHANGED: &str = "the retained evidence differs from its ledger digest";
pub const JOURNAL_HEADING: &str = "Journals, newest first:\n";

pub fn header_value(key: &str) -> String {
    format!("{key} must be nonempty and on one line")
}

pub fn header_repeat(key: &str) -> String {
    format!("the header repeats {key}")
}

pub fn header_unknown(key: &str) -> String {
    format!("unknown memory header field {key:?}")
}

pub fn header_quoted(error: impl std::fmt::Display) -> String {
    format!("invalid quoted header field: {error}")
}

pub fn conflict(expected: u32) -> String {
    format!("memory revision conflict: current version is {expected}")
}

pub fn missing(name: &str) -> String {
    format!("no such memory document or revision: {name}")
}

pub fn utf8(error: impl std::fmt::Display) -> String {
    format!("memory Markdown must be UTF-8: {error}")
}

pub fn saved(tool: &str, path: impl std::fmt::Display) -> String {
    format!("=== {tool} {path} (project memory saved) ===")
}

pub fn saved_unrecorded(error: impl std::fmt::Display) -> String {
    format!("memory was saved, but recording its change failed: {error}")
}

pub fn missing_evidence(path: impl std::fmt::Display) -> String {
    format!("retained evidence is missing: {path}")
}

pub fn paths_old(machine: impl std::fmt::Display) -> String {
    format!("update {machine} to read project memory paths")
}
