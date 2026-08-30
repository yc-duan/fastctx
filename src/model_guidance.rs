//! Single-source prose for routing local-file work into the model-visible tool surface.

use crate::server_manifest::EnabledTools;

/// Positive routing shared by the host guidance file and every fresh MCP connection.
pub(crate) const LOCAL_FILE_ROUTE_GUIDANCE: &str = concat!(
    "Use FastCtx file tools directly for local-file operations, including when a\n",
    "local reference is URI-shaped; pass the equivalent plain absolute filesystem path."
);

/// Positive routing for a set that publishes no file tool; the URI clause has no
/// subject there, and advertising an absent capability teaches a route that does
/// not exist.
pub(crate) const LOCAL_SHELL_ROUTE_GUIDANCE: &str = concat!(
    "Use FastCtx shell tools directly for local command work; they run POSIX bash\n",
    "on every platform."
);

/// Shared path-shape contract used by every file tool's path field.
pub(crate) const LOCAL_PATH_INPUT_GUIDANCE: &str = concat!(
    "Plain absolute local filesystem path. When the source reference is URI-shaped, ",
    "use its equivalent local absolute path."
);

/// Opening sentence of the file-inspection tool's description.
///
/// It leads with what the tool is over a local filesystem path, because that is the
/// sentence a routing decision is made on: `read_mcp_resource` competes for the same
/// intent and the discriminating fact is the input shape, not the verb (2026-08-08).
const INSPECT_TOOL_SUMMARY: &str =
    "Inspect one local filesystem path as text, an image, a PDF, or raw bytes.";

const INSPECT_TOOL_DETAILS: &str = concat!(
    "Text returns 1-based `N<tab>content` lines, as much of the file as\n",
    "the output budget holds. Continue text or hex from the last delivered line plus one.\n",
    "Images (PNG/JPG/GIF/WebP/BMP) are\n",
    "shown to you visually. PDFs return the selected pages' text layer or those\n",
    "pages rendered as images; image mode defaults to 4 pages. Continue a PDF with the\n",
    "page after the last delivered page. view=\"hex\" dumps any file's raw bytes. Text\n",
    "output is always UTF-8."
);

pub(crate) fn local_path_description(context: &str) -> String {
    format!("{LOCAL_PATH_INPUT_GUIDANCE} {context}")
}

pub(crate) fn inspect_tool_description() -> String {
    format!(
        "{} {} {INSPECT_TOOL_DETAILS}",
        INSPECT_TOOL_SUMMARY.replace('\n', " "),
        LOCAL_FILE_ROUTE_GUIDANCE.replace('\n', " ")
    )
}

pub(crate) fn server_instructions(tools: EnabledTools) -> String {
    if tools.files_enabled() {
        LOCAL_FILE_ROUTE_GUIDANCE.replace('\n', " ")
    } else {
        LOCAL_SHELL_ROUTE_GUIDANCE.replace('\n', " ")
    }
}

/// File tools able to read a background job's log file, in manifest order.
const LOG_READER_TOOLS: [&str; 2] = ["inspect_local_file", "grep"];

const RUN_BACKGROUND_HEAD: &str = concat!(
    "Start a bash command as a background job and return its job_id\n",
    "immediately. Use for builds, tests, servers, or anything that may outlast\n",
    "run's 240000 ms ceiling. Jobs survive server and agent restarts; their\n",
    "output and exit code stay retrievable by job_id. Check on it with\n",
    "job_output; stop with job_kill; rediscover past jobs with job_list. There\n",
    "is no timeout: a job runs until it exits or is killed. Everything it\n",
    "prints is kept in a plain log file whose path is returned here"
);

const RUN_BACKGROUND_TAIL: &str = concat!(
    "Background status refreshes only when another FastCtx tool returns; it is not\n",
    "a push notification, so keep working and check back when useful."
);

const JOB_OUTPUT_HEAD: &str = concat!(
    "Query a background job: its status plus output after after_seq (or after\n",
    "the session cursor when omitted). Works for jobs started in earlier sessions.\n",
    "Long output is windowed: the first lines on the initial call and newest lines\n",
    "that fit. Sequence numbers in the head note map directly to the plain log file\n",
    "on disk"
);

const JOB_OUTPUT_TAIL: &str = concat!(
    "The call\n",
    "blocks up to wait_ms; use 0 for a snapshot, and raise it only when you have\n",
    "nothing else to do."
);

/// Names the enabled file tools that can read a job log, or nothing when none are published.
///
/// Naming a tool the target does not publish teaches the model a route that does not exist,
/// and co-occurrence beats negation, so this clause follows the enabled set (2026-08-30).
fn log_readers(tools: EnabledTools) -> Option<String> {
    let names = LOG_READER_TOOLS
        .into_iter()
        .filter(|name| tools.contains(name))
        .collect::<Vec<_>>();
    (!names.is_empty()).then(|| names.join(" or "))
}

pub(crate) fn run_background_tool_description(tools: EnabledTools) -> String {
    let clause = match log_readers(tools) {
        Some(readers) => format!(";\n{readers} that path for anything job_output does not show.\n"),
        None => ".\n".to_string(),
    };
    format!("{RUN_BACKGROUND_HEAD}{clause}{RUN_BACKGROUND_TAIL}")
}

pub(crate) fn job_output_tool_description(tools: EnabledTools) -> String {
    let clause = match log_readers(tools) {
        Some(readers) => format!(", so {readers} that path for omitted output. "),
        None => ". ".to_string(),
    };
    format!("{JOB_OUTPUT_HEAD}{clause}{JOB_OUTPUT_TAIL}")
}

const REPLACE_TOOL_HEAD: &str = concat!(
    "Batch find-and-replace across a file or directory (Rust regex; no lookaround).\n",
    "A reference to an undefined capture group is rejected before any write. To\n",
    "delete whole lines, include \\n in the pattern. Matching is leftmost-first and\n",
    "non-overlapping; "
);

const REPLACE_TOOL_TAIL: &str = concat!(
    "`^`/`$` anchor the whole file by default — use (?m) for\n",
    "per-line anchors. Respects .gitignore; skips .git and binaries; files whose\n",
    "encoding cannot be determined are skipped and listed. Each file is written\n",
    "atomically with a concurrent-modification check, preserving its original\n",
    "encoding, BOM, and line endings."
);

/// The anchor contrast only reads as a warning to a model that also holds `grep`, and
/// naming an unpublished tool teaches a route that does not exist, so it is conditional.
pub(crate) fn replace_tool_description(tools: EnabledTools) -> String {
    let contrast = if tools.contains("grep") {
        "unlike grep, "
    } else {
        ""
    };
    format!("{REPLACE_TOOL_HEAD}{contrast}{REPLACE_TOOL_TAIL}")
}
