//! Text, image, PDF, and raw-byte dispatch for the read tool.

mod batch;
mod hex_file;
mod image_file;
#[cfg(feature = "pdf")]
mod pdf;
#[cfg(not(feature = "pdf"))]
#[path = "pdf_disabled.rs"]
mod pdf;
#[cfg(feature = "pdf")]
mod pdf_engine;
mod text_file;

use crate::binary::detect_binary_type;
use crate::budget::{READ_TOKEN_BUDGET_ENV, tool_token_budget};
use crate::model::ToolResponse;
use crate::paths::{
    ReadScope, absolute_path_required_message, canonical_existing, display_path, io_error_message,
    missing_read_file_message, parse_input_path,
};
use schemars::JsonSchema;
use serde::Deserialize;
use std::fs;
use std::io::Read;

/// Line window for a text read that omits `limit`: unbounded, so the token budget is the only
/// ceiling on how much one call returns.
///
/// A second fixed window would silently cap every default read far below the configured budget,
/// forcing continuation calls the budget was raised to avoid. Do not reintroduce a numeric default
/// here; page explicitly with `limit` instead. Locked by
/// `read_contract::default_text_read_is_bounded_only_by_the_token_budget`. (2026-07-25)
const UNBOUNDED_LINE_LIMIT: usize = usize::MAX;
/// Line window for a hex read that omits `limit`.
///
/// Hex keeps a fixed default where text does not: a dump exists to inspect a byte range rather
/// than to deliver a whole binary, and its candidate window is estimated from the budget with a
/// deliberately loose factor, so an unbounded default would buffer hundreds of thousands of
/// rendered lines before the budget trimmed them back. (2026-07-25)
const DEFAULT_HEX_LINE_LIMIT: usize = 2_000;
const MAX_LINE_CHARS: usize = 2_000;
const TOTAL_COUNT_SIZE_LIMIT: u64 = 64 * 1024 * 1024;

/// Automatic read dispatch or raw-byte viewing.
#[derive(Clone, Copy, Debug, Default, Deserialize, JsonSchema, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
enum ViewMode {
    /// Select the text, image, or PDF channel from the file type.
    #[default]
    Auto,
    /// Return a paged hexadecimal dump of the raw bytes.
    Hex,
}

/// Parameters for the read tool; offset is a one-based line number.
#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReadRequest {
    /// The absolute path to the file to read. Both / and \ are accepted. Mutually exclusive with files.
    pub file_path: Option<String>,
    /// Batch form: an array of {"path", "offset"?, "limit"?, "encoding"?} objects for
    /// reading 1-32 text files in one call. Each entry behaves like a single-file text read;
    /// results are packed in request order. Mutually exclusive with file_path and with the
    /// top-level offset/limit/encoding/pages/pdf_mode/view parameters.
    #[schemars(length(min = 1, max = 32))]
    pub files: Option<Vec<BatchReadEntry>>,
    /// The 1-based line number to start reading from. Use for paging through large files.
    #[schemars(range(min = 1))]
    pub offset: Option<usize>,
    /// The number of lines to read. Omit to read as much as the output budget holds.
    #[schemars(range(min = 1))]
    pub limit: Option<usize>,
    /// Page range for PDF files, e.g. "1-5", "3", "10-20". Max 20 pages per call. Required in text mode for PDFs with more than 10 pages.
    pub pages: Option<String>,
    /// PDF only: "text" (default) returns the selected pages' text layer; "image" returns each selected page rendered as a PNG image.
    #[schemars(with = "Option<pdf::PdfMode>")]
    pub pdf_mode: Option<String>,
    /// Text files only. Known source encoding as a WHATWG label, e.g. "gbk", "shift_jis", "big5", "euc-kr", "windows-1252", "utf-16le", plus "utf-32le"/"utf-32be". Selects how source bytes are decoded; output is always UTF-8. Omit for auto-detection; set it when you know the source encoding or the tool reports an ambiguous or undecodable encoding.
    pub encoding: Option<String>,
    /// "auto" (default) picks the channel by file type; "hex" returns a paged hex dump of the raw bytes of any file — the way to inspect binary files.
    #[schemars(with = "Option<ViewMode>")]
    pub view: Option<String>,
}

/// One text file in a batch read request.
#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BatchReadEntry {
    /// Absolute path of the text file. Both / and \ are accepted.
    pub path: String,
    /// The 1-based line number to start reading from.
    #[schemars(range(min = 1))]
    pub offset: Option<usize>,
    /// Maximum lines to read from this file in this call. Omit to let the shared budget decide.
    #[schemars(range(min = 1))]
    pub limit: Option<usize>,
    /// Known source encoding for this file, using the same labels as single-file read.
    pub encoding: Option<String>,
}

/// Reads text, images, PDFs, or raw bytes and surfaces every expected failure explicitly.
pub fn read_file(request: ReadRequest) -> ToolResponse {
    read_file_with_scope(request, &ReadScope::unrestricted())
}

pub(crate) fn read_file_with_scope(request: ReadRequest, scope: &ReadScope) -> ToolResponse {
    match (request.file_path.as_deref(), request.files.as_ref()) {
        (Some(_), Some(_)) | (None, None) => {
            return ToolResponse::error("Provide exactly one of file_path or files.");
        }
        (None, Some(_)) => return batch::read_text_files(request, scope),
        (Some(_), None) => {}
    }
    let file_path = request
        .file_path
        .as_deref()
        .expect("single-file shape was validated");
    let parsed = parse_input_path(file_path);
    if !parsed.is_absolute() {
        if scope.is_restricted() {
            return ToolResponse::error(absolute_path_required_message(file_path));
        }
        return ToolResponse::error(missing_read_file_message(file_path));
    }
    let authorized = match scope.authorize(&parsed) {
        Ok(path) => path,
        Err(message) => return ToolResponse::error(message),
    };
    let metadata = match fs::metadata(&authorized) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return ToolResponse::error(missing_read_file_message(file_path));
        }
        Err(error) => return ToolResponse::error(io_error_message(&authorized, &error)),
    };
    let path = canonical_existing(&authorized).unwrap_or(authorized);
    let path_display = display_path(&path);
    if metadata.is_dir() {
        return ToolResponse::error(format!(
            "{path_display} is a directory, not a file. Use the glob tool to list its contents."
        ));
    }
    if !metadata.is_file() {
        return ToolResponse::error(format!(
            "Cannot read non-regular file: {path_display}. Only regular files are supported."
        ));
    }
    let mut prefix = Vec::new();
    let prefix_result =
        fs::File::open(&path).and_then(|file| file.take(8 * 1024).read_to_end(&mut prefix));
    if let Err(error) = prefix_result {
        return ToolResponse::error(io_error_message(&path, &error));
    }

    let view = match parse_view(request.view.as_deref()) {
        Ok(view) => view,
        Err(message) => return ToolResponse::error(message),
    };
    if view == ViewMode::Hex {
        for (parameter, present) in [
            ("pdf_mode", request.pdf_mode.is_some()),
            ("pages", request.pages.is_some()),
            ("encoding", request.encoding.is_some()),
        ] {
            if present {
                return ToolResponse::error(format!(
                    "The {parameter} parameter cannot be combined with view=\"hex\"."
                ));
            }
        }
        let budget = match tool_token_budget(READ_TOKEN_BUDGET_ENV) {
            Ok(budget) => budget,
            Err(message) => return ToolResponse::error(message),
        };
        return hex_file::read_hex_file(&path, request.offset, request.limit, budget);
    }

    if pdf::is_pdf(&path, &prefix) {
        if request.encoding.is_some() {
            return ToolResponse::error("The encoding parameter only applies to text files.");
        }
        let mode = match pdf::parse_pdf_mode(request.pdf_mode.as_deref()) {
            Ok(mode) => mode,
            Err(message) => return ToolResponse::error(message),
        };
        let budget = if mode == pdf::PdfMode::Text {
            match tool_token_budget(READ_TOKEN_BUDGET_ENV) {
                Ok(budget) => Some(budget),
                Err(message) => return ToolResponse::error(message),
            }
        } else {
            None
        };
        return pdf::read_pdf(&path, request.pages.as_deref(), mode, budget);
    }
    if request.pages.is_some() {
        return ToolResponse::error("The pages parameter only applies to PDF files.");
    }
    if request.pdf_mode.is_some() {
        return ToolResponse::error("The pdf_mode parameter only applies to PDF files.");
    }
    if image_file::detect_image_mime(&path, &prefix).is_some() {
        if request.encoding.is_some() {
            return ToolResponse::error("The encoding parameter only applies to text files.");
        }
        return image_file::read_image(&path);
    }
    let budget = match tool_token_budget(READ_TOKEN_BUDGET_ENV) {
        Ok(budget) => budget,
        Err(message) => return ToolResponse::error(message),
    };
    text_file::read_text_file(
        &path,
        &path_display,
        request.offset,
        request.limit,
        request.encoding.as_deref(),
        detect_binary_type(&prefix),
        budget,
    )
}

fn parse_view(value: Option<&str>) -> Result<ViewMode, String> {
    match value {
        None | Some("auto") => Ok(ViewMode::Auto),
        Some("hex") => Ok(ViewMode::Hex),
        Some(value) => Err(format!(
            "Invalid view value \"{value}\". Use \"auto\" or \"hex\"."
        )),
    }
}

fn binary_error(path_display: &str, binary_type: Option<&str>) -> ToolResponse {
    ToolResponse::error(binary_error_message(path_display, binary_type))
}

fn binary_error_message(path_display: &str, binary_type: Option<&str>) -> String {
    let kind = binary_type.map_or_else(String::new, |kind| format!(" (looks like {kind})"));
    format!(
        "Cannot read binary file as text: {path_display}{kind}. Use view=\"hex\" to inspect its raw bytes."
    )
}

#[cfg(test)]
mod tests {
    use super::{BatchReadEntry, ReadRequest, read_file_with_scope};
    use crate::ToolContent;
    use crate::paths::{ReadScope, display_path};
    use std::fs;

    fn single(path: &std::path::Path) -> ReadRequest {
        ReadRequest {
            file_path: Some(display_path(path)),
            files: None,
            offset: None,
            limit: None,
            pages: None,
            pdf_mode: None,
            encoding: None,
            view: None,
        }
    }

    fn response_text(response: crate::ToolResponse) -> String {
        let [ToolContent::Text(text)] = response.content.as_slice() else {
            panic!("expected one text response: {response:?}");
        };
        text.clone()
    }

    #[test]
    fn scoped_read_allows_direct_files_and_denies_outside_and_lexical_prefixes() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().join("work");
        let sibling = fixture.path().join("work-secret");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&sibling).unwrap();
        let inside = root.join("inside.txt");
        let outside = sibling.join("outside.txt");
        fs::write(&inside, b"allowed-content").unwrap();
        fs::write(&outside, b"denied-content").unwrap();
        let scope = ReadScope::from_allow_roots(std::slice::from_ref(&root)).unwrap();

        let allowed = read_file_with_scope(single(&inside), &scope);
        assert!(!allowed.is_error, "{allowed:?}");
        assert!(response_text(allowed).contains("allowed-content"));

        for denied_path in [&outside, &sibling.join("outside.txt")] {
            let response = read_file_with_scope(single(denied_path), &scope);
            assert!(response.is_error, "{response:?}");
            let text = response_text(response);
            assert!(text.starts_with("Permission denied: "), "{text}");
            assert!(!text.contains("denied-content"), "{text}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn scoped_read_denies_static_file_symlink_without_opening_target() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().join("allowed");
        let outside = fixture.path().join("outside.txt");
        fs::create_dir(&root).unwrap();
        fs::write(&outside, b"secret-target-content").unwrap();
        let link = root.join("outside-link.txt");
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        let scope = ReadScope::from_allow_roots(std::slice::from_ref(&root)).unwrap();

        let response = read_file_with_scope(single(&link), &scope);
        assert!(response.is_error, "{response:?}");
        let text = response_text(response);
        assert!(text.starts_with("Permission denied: "), "{text}");
        assert!(!text.contains("secret-target-content"), "{text}");
    }

    #[test]
    fn scoped_batch_keeps_allowed_entries_and_reports_denied_entries_inline() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().join("allowed");
        let outside = fixture.path().join("outside.txt");
        fs::create_dir(&root).unwrap();
        let inside = root.join("inside.txt");
        fs::write(&inside, b"allowed-batch-content").unwrap();
        fs::write(&outside, b"denied-batch-content").unwrap();
        let scope = ReadScope::from_allow_roots(std::slice::from_ref(&root)).unwrap();
        let request = ReadRequest {
            file_path: None,
            files: Some(vec![
                BatchReadEntry {
                    path: display_path(&inside),
                    offset: None,
                    limit: None,
                    encoding: None,
                },
                BatchReadEntry {
                    path: display_path(&outside),
                    offset: None,
                    limit: None,
                    encoding: None,
                },
            ]),
            offset: None,
            limit: None,
            pages: None,
            pdf_mode: None,
            encoding: None,
            view: None,
        };
        let response = read_file_with_scope(request, &scope);
        assert!(!response.is_error, "{response:?}");
        let text = response_text(response);
        assert!(text.contains("allowed-batch-content"), "{text}");
        assert!(text.contains("Permission denied: "), "{text}");
        assert!(!text.contains("denied-batch-content"), "{text}");
    }

    #[test]
    fn restricted_relative_single_and_batch_requests_use_stable_absolute_errors() {
        let fixture = tempfile::tempdir().unwrap();
        let scope = ReadScope::from_allow_roots(&[fixture.path().to_path_buf()]).unwrap();
        let single_response = read_file_with_scope(
            ReadRequest {
                file_path: Some("relative.txt".to_string()),
                files: None,
                offset: None,
                limit: None,
                pages: None,
                pdf_mode: None,
                encoding: None,
                view: None,
            },
            &scope,
        );
        assert_eq!(
            response_text(single_response),
            "Path must be absolute: relative.txt"
        );

        let batch_response = read_file_with_scope(
            ReadRequest {
                file_path: None,
                files: Some(vec![BatchReadEntry {
                    path: "relative.txt".to_string(),
                    offset: None,
                    limit: None,
                    encoding: None,
                }]),
                offset: None,
                limit: None,
                pages: None,
                pdf_mode: None,
                encoding: None,
                view: None,
            },
            &scope,
        );
        let text = response_text(batch_response);
        assert!(
            text.contains("Path must be absolute: relative.txt"),
            "{text}"
        );
        assert!(!text.contains("session working directory"), "{text}");
    }

    #[test]
    fn restricted_batch_denied_aliases_remain_inline_instead_of_duplicate_errors() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().join("allowed");
        let outside = fixture.path().join("outside.txt");
        fs::create_dir(&root).unwrap();
        fs::write(&outside, b"must-not-leak").unwrap();
        let alias = root.join("..").join("outside.txt");
        let scope = ReadScope::from_allow_roots(std::slice::from_ref(&root)).unwrap();
        let request = ReadRequest {
            file_path: None,
            files: Some(vec![
                BatchReadEntry {
                    path: display_path(&outside),
                    offset: None,
                    limit: None,
                    encoding: None,
                },
                BatchReadEntry {
                    path: display_path(&alias),
                    offset: None,
                    limit: None,
                    encoding: None,
                },
            ]),
            offset: None,
            limit: None,
            pages: None,
            pdf_mode: None,
            encoding: None,
            view: None,
        };
        let response = read_file_with_scope(request, &scope);
        assert!(!response.is_error, "{response:?}");
        let text = response_text(response);
        assert_eq!(text.matches("Permission denied: ").count(), 2, "{text}");
        assert!(!text.contains("must-not-leak"), "{text}");
        assert!(!text.contains("Duplicate path"), "{text}");
    }
}
