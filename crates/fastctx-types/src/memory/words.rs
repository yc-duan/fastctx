//! Project-memory replies and refusal text.

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
pub const WRITER_GONE: &str = "the writing conversation no longer exists";
pub const WRITER_UNANSWERED: &str = "the conversation's machine did not identify its writer";
pub const WRITER_INVISIBLE: &str = "the remote writing conversation is not visible";
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
pub const LEGACY: &str = "This journal interface was retired; use the history files listed in your context.";


pub const HEADER_QUOTE: &str = "a quoted header field needs its closing quote";

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
