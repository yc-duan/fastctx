//! PDF request surface for builds compiled without the optional `pdf` feature.

use crate::budget::TokenBudget;
use crate::model::ToolResponse;
use schemars::JsonSchema;
use serde::Deserialize;
use std::path::Path;

/// PDF response channel retained in the public schema for no-PDF builds.
#[derive(Clone, Copy, Debug, Default, Deserialize, JsonSchema, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub(super) enum PdfMode {
    /// Return the selected pages' text layer.
    #[default]
    Text,
    /// Return rendered page images.
    Image,
}

pub(super) fn is_pdf(path: &Path, bytes: &[u8]) -> bool {
    bytes.starts_with(b"%PDF")
        || path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"))
}

pub(super) fn parse_pdf_mode(value: Option<&str>) -> Result<PdfMode, String> {
    match value {
        None | Some("text") => Ok(PdfMode::Text),
        Some("image") => Ok(PdfMode::Image),
        Some(value) => Err(format!(
            "Invalid pdf_mode value \"{value}\". Use \"text\" or \"image\"."
        )),
    }
}

pub(super) fn read_pdf(
    _path: &Path,
    _pages_value: Option<&str>,
    _mode: PdfMode,
    _text_budget: Option<TokenBudget>,
) -> ToolResponse {
    ToolResponse::error(
        "PDF support is unavailable: could not load the bundled PDF engine (this binary was built without the pdf feature). Other file types are unaffected.",
    )
}
