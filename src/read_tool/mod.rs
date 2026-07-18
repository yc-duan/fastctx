//! Text, image, PDF, and raw-byte dispatch for the read tool.

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
    canonical_existing, display_path, io_error_message, missing_read_file_message, parse_input_path,
};
use schemars::JsonSchema;
use serde::Deserialize;
use std::fs;
use std::io::Read;

const DEFAULT_LINE_LIMIT: usize = 2_000;
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
pub struct ReadRequest {
    /// The absolute path to the file to read. Both / and \ are accepted.
    pub file_path: String,
    /// The 1-based line number to start reading from. Use for paging through large files.
    #[schemars(range(min = 1))]
    pub offset: Option<usize>,
    /// The number of lines to read (default 2000).
    #[schemars(range(min = 1))]
    pub limit: Option<usize>,
    /// Page range for PDF files, e.g. "1-5", "3", "10-20". Max 20 pages per call. Required for PDFs with more than 10 pages.
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

/// Reads text, images, PDFs, or raw bytes and surfaces every expected failure explicitly.
pub fn read_file(request: ReadRequest) -> ToolResponse {
    let parsed = parse_input_path(&request.file_path);
    if !parsed.is_absolute() {
        return ToolResponse::error(missing_read_file_message(&request.file_path));
    }
    let metadata = match fs::metadata(&parsed) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return ToolResponse::error(missing_read_file_message(&request.file_path));
        }
        Err(error) => return ToolResponse::error(io_error_message(&parsed, &error)),
    };
    let path = canonical_existing(&parsed).unwrap_or(parsed);
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
    let kind = binary_type.map_or_else(String::new, |kind| format!(" (looks like {kind})"));
    ToolResponse::error(format!(
        "Cannot read binary file as text: {path_display}{kind}. Use view=\"hex\" to inspect its raw bytes."
    ))
}
