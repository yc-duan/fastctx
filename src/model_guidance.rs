//! Single-source prose for routing local-file work into the model-visible tool surface.

/// Positive routing shared by the host guidance file and every fresh MCP connection.
pub(crate) const LOCAL_FILE_ROUTE_GUIDANCE: &str = concat!(
    "Use FastCtx file tools directly for local-file operations, including when a\n",
    "local reference is URI-shaped; pass the equivalent plain absolute filesystem path."
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
    "output is always UTF-8; when auto-detection is not confident it returns an\n",
    "error listing candidate encodings instead of guessed text, so pass encoding\n",
    "only then."
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

pub(crate) fn server_instructions() -> String {
    LOCAL_FILE_ROUTE_GUIDANCE.replace('\n', " ")
}
