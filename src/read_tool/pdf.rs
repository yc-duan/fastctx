//! PDF page selection, text extraction, and 150-DPI page rendering.

use crate::budget::{TokenBudget, assemble_text, estimate_tokens};
use crate::model::{ImageDetail, ToolContent, ToolResponse};
use crate::read_tool::pdf_engine::{PdfOperationError, pdfium_session, run_pdf_operation};
use base64::Engine;
use image::ImageFormat;
use pdfium_render::prelude::{PdfDocument, PdfRenderConfig, PdfiumError, PdfiumInternalError};
use schemars::JsonSchema;
use serde::Deserialize;
use std::io::Cursor;
use std::path::Path;

const DEFAULT_MAX_PAGES: usize = 10;
const DEFAULT_IMAGE_PAGES: usize = 4;
const MAX_PAGES_PER_CALL: usize = 20;
const RENDER_DPI: f32 = 150.0;
const MAX_RENDER_DIMENSION: u64 = 16_384;
const MAX_PAGE_PIXELS: u64 = 32_000_000;
const MAX_CALL_PIXELS: u64 = 64_000_000;
const MAX_IMAGE_PAYLOAD_BYTES: usize = 8 * 1024 * 1024;

/// Mutually exclusive PDF response channels.
#[derive(Clone, Copy, Debug, Default, Deserialize, JsonSchema, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub(super) enum PdfMode {
    /// Return only the text layer of the selected pages.
    #[default]
    Text,
    /// Return only full-page PNG images for the selected pages.
    Image,
}

#[derive(Debug)]
struct TextPage {
    number: usize,
    text: String,
}

#[derive(Clone, Copy, Debug)]
struct RenderPlan {
    number: usize,
    width: i32,
    height: i32,
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
    path: &Path,
    pages_value: Option<&str>,
    mode: PdfMode,
    text_budget: Option<TokenBudget>,
) -> ToolResponse {
    let path = path.to_path_buf();
    let path_display = crate::paths::display_path(&path);
    let pages_value = pages_value.map(str::to_string);
    match run_pdf_operation(move || {
        read_pdf_inner(&path, pages_value.as_deref(), mode, text_budget)
    }) {
        Ok(response) => response,
        Err(PdfOperationError::TimedOut) => ToolResponse::error(format!(
            "PDF operation on {path_display} timed out and was aborted. The file may be malformed; other file types are unaffected."
        )),
        Err(PdfOperationError::Unavailable(reason)) => pdf_engine_error(&reason),
    }
}

fn read_pdf_inner(
    path: &Path,
    pages_value: Option<&str>,
    mode: PdfMode,
    text_budget: Option<TokenBudget>,
) -> ToolResponse {
    let (_operation, pdfium) = match pdfium_session() {
        Ok(session) => session,
        Err(reason) => return pdf_engine_error(&reason),
    };
    let document = match pdfium.load_pdf_from_file(path, None) {
        Ok(document) => document,
        Err(error) => return pdf_load_error(error),
    };
    let total_pages = document.pages().len() as usize;
    if total_pages == 0 {
        return corrupted_pdf();
    }
    let selected = match parse_pages(pages_value, total_pages, mode) {
        Ok(selected) => selected,
        Err(message) => return ToolResponse::error(message),
    };

    match mode {
        PdfMode::Text => read_pdf_text(
            &document,
            &selected,
            total_pages,
            text_budget.expect("text mode always receives a token budget"),
        ),
        PdfMode::Image => read_pdf_images(&document, &selected, total_pages),
    }
}

fn read_pdf_text(
    document: &PdfDocument<'_>,
    selected: &[usize],
    total_pages: usize,
    budget: TokenBudget,
) -> ToolResponse {
    let mut pages = Vec::with_capacity(selected.len());
    for page_number in selected {
        let page = match document.pages().get((*page_number - 1) as i32) {
            Ok(page) => page,
            Err(_) => return corrupted_pdf(),
        };
        let text = match page.text() {
            Ok(text) => normalize_pdf_text(&text.all()),
            Err(_) => return corrupted_pdf(),
        };
        pages.push(TextPage {
            number: *page_number,
            text,
        });
    }
    format_text_pages(&pages, total_pages, budget)
}

fn format_text_pages(pages: &[TextPage], total_pages: usize, budget: TokenBudget) -> ToolResponse {
    let selected_all_no_text = pages.iter().all(|page| page.text.trim().is_empty());
    for shown in (1..=pages.len()).rev() {
        let output = render_text_output(&pages[..shown], total_pages, selected_all_no_text, false);
        if estimate_tokens(&output) <= budget.value {
            return ToolResponse::text(output);
        }
    }

    truncate_first_text_page(&pages[0], total_pages, budget)
}

fn truncate_first_text_page(
    page: &TextPage,
    total_pages: usize,
    budget: TokenBudget,
) -> ToolResponse {
    if page.text.trim().is_empty() {
        return budget_too_small(budget);
    }
    let lines = page.text.split('\n').collect::<Vec<_>>();
    let mut low = 0_usize;
    let mut high = lines.len().saturating_sub(1);
    let mut best = None;
    while low <= high {
        let count = low + (high - low) / 2;
        let text = lines[..count].join("\n");
        let truncated = TextPage {
            number: page.number,
            text,
        };
        let output = render_text_output(&[truncated], total_pages, false, true);
        if estimate_tokens(&output) <= budget.value {
            best = Some(output);
            low = count.saturating_add(1);
        } else if count == 0 {
            break;
        } else {
            high = count - 1;
        }
    }

    match best {
        Some(output) => ToolResponse::text(output),
        None => budget_too_small(budget),
    }
}

fn render_text_output(
    pages: &[TextPage],
    total_pages: usize,
    selected_all_no_text: bool,
    first_page_truncated: bool,
) -> String {
    let body = pages
        .iter()
        .map(text_page_block)
        .collect::<Vec<_>>()
        .join("\n\n");
    let mut notes = Vec::new();
    if selected_all_no_text {
        notes.push(
            "(Note: no text layer in the selected pages; use pdf_mode=\"image\" to view rendered pages.)"
                .to_string(),
        );
    }
    if first_page_truncated {
        notes.push(format!(
            "(Note: page {} text truncated at the token budget; use pdf_mode=\"image\" to view the full page.)",
            pages[0].number
        ));
    }
    notes.push(pdf_terminal_note(
        pages[0].number,
        pages.last().expect("text response has a page").number,
        total_pages,
        PdfMode::Text,
        pages.len(),
    ));
    assemble_text(&[body], &notes)
}

fn text_page_block(page: &TextPage) -> String {
    if page.text.trim().is_empty() {
        format!("=== Page {} === (no text layer)", page.number)
    } else {
        format!("=== Page {} ===\n{}", page.number, page.text)
    }
}

fn read_pdf_images(
    document: &PdfDocument<'_>,
    selected: &[usize],
    total_pages: usize,
) -> ToolResponse {
    let plans = match preflight_render_plans(document, selected) {
        Ok(plans) => plans,
        Err(response) => return response,
    };
    collect_encoded_images(&plans, total_pages, MAX_IMAGE_PAYLOAD_BYTES, |plan| {
        encode_page_png(document, plan)
    })
}

fn preflight_render_plans(
    document: &PdfDocument<'_>,
    selected: &[usize],
) -> Result<Vec<RenderPlan>, ToolResponse> {
    let mut plans = Vec::with_capacity(selected.len());
    let mut call_pixels = 0_u64;
    for page_number in selected {
        let page = document
            .pages()
            .get((*page_number - 1) as i32)
            .map_err(|_| corrupted_pdf())?;
        let (width, height, pixels) =
            render_dimensions(*page_number, page.width().value, page.height().value)?;
        call_pixels = call_pixels.saturating_add(pixels);
        if call_pixels > MAX_CALL_PIXELS {
            return Err(ToolResponse::error(format!(
                "Cannot render selected PDF pages: combined 150 DPI size exceeds the {MAX_CALL_PIXELS}-pixel safety limit. Select fewer pages."
            )));
        }
        plans.push(RenderPlan {
            number: *page_number,
            width,
            height,
        });
    }
    Ok(plans)
}

fn collect_encoded_images(
    plans: &[RenderPlan],
    total_pages: usize,
    payload_limit: usize,
    mut encode: impl FnMut(&RenderPlan) -> Result<String, ToolResponse>,
) -> ToolResponse {
    let mut content = Vec::with_capacity(plans.len() + 1);
    let mut payload_bytes = 0_usize;
    for plan in plans {
        let data = match encode(plan) {
            Ok(data) => data,
            Err(response) => return response,
        };
        if payload_bytes.saturating_add(data.len()) > payload_limit {
            if content.is_empty() {
                return ToolResponse::error(format!(
                    "Cannot return PDF page {} as an image: the encoded image exceeds the 8 MiB payload safety limit. Use pdf_mode=\"text\" for this page.",
                    plan.number
                ));
            }
            break;
        }
        payload_bytes = payload_bytes.saturating_add(data.len());
        content.push(ToolContent::Image {
            data,
            mime_type: "image/png".to_string(),
            detail: Some(ImageDetail::High),
        });
    }
    let delivered = content.len();
    let first = plans[0].number;
    let last = plans[delivered - 1].number;
    content.push(ToolContent::Text(pdf_terminal_note(
        first,
        last,
        total_pages,
        PdfMode::Image,
        delivered,
    )));
    ToolResponse {
        content,
        is_error: false,
    }
}

fn encode_page_png(document: &PdfDocument<'_>, plan: &RenderPlan) -> Result<String, ToolResponse> {
    let page = document
        .pages()
        .get((plan.number - 1) as i32)
        .map_err(|_| corrupted_pdf())?;
    let bitmap = page
        .render_with_config(&PdfRenderConfig::new().set_target_size(plan.width, plan.height))
        .map_err(|_| corrupted_pdf())?;
    let image = bitmap.as_image().map_err(|_| corrupted_pdf())?;
    let mut png = Cursor::new(Vec::new());
    image
        .write_to(&mut png, ImageFormat::Png)
        .map_err(|_| corrupted_pdf())?;
    Ok(base64::engine::general_purpose::STANDARD.encode(png.into_inner()))
}

fn pdf_terminal_note(
    first: usize,
    last: usize,
    total: usize,
    mode: PdfMode,
    delivered: usize,
) -> String {
    let span = page_span(first, last);
    let verb = match mode {
        PdfMode::Text => "shown",
        PdfMode::Image => "rendered",
    };
    if last == total {
        format!("(Complete: {span} of {total} {verb}.)")
    } else {
        let next_start = last + 1;
        let next_end = next_start
            .saturating_add(delivered.saturating_sub(1))
            .min(total);
        let next = if next_start == next_end {
            next_start.to_string()
        } else {
            format!("{next_start}-{next_end}")
        };
        format!("(Partial: {span} of {total} {verb}. Continue with pages=\"{next}\".)")
    }
}

fn page_span(first: usize, last: usize) -> String {
    if first == last {
        format!("page {first}")
    } else {
        format!("pages {first}-{last}")
    }
}

fn render_dimensions(
    page_number: usize,
    width_points: f32,
    height_points: f32,
) -> Result<(i32, i32, u64), ToolResponse> {
    let width = f64::from(width_points) * f64::from(RENDER_DPI) / 72.0;
    let height = f64::from(height_points) * f64::from(RENDER_DPI) / 72.0;
    if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 {
        return Err(ToolResponse::error(format!(
            "Cannot render PDF page {page_number}: invalid page dimensions. Repair or regenerate the PDF externally."
        )));
    }
    let width = width.round().max(1.0) as u64;
    let height = height.round().max(1.0) as u64;
    let pixels = width.saturating_mul(height);
    if width > MAX_RENDER_DIMENSION || height > MAX_RENDER_DIMENSION || pixels > MAX_PAGE_PIXELS {
        return Err(ToolResponse::error(format!(
            "Cannot render PDF page {page_number}: dimensions {width}x{height} pixels at 150 DPI exceed the rendering safety limits (max {MAX_RENDER_DIMENSION} pixels per side and {MAX_PAGE_PIXELS} pixels per page). Reduce the page size externally."
        )));
    }
    Ok((width as i32, height as i32, pixels))
}

fn parse_pages(
    value: Option<&str>,
    total_pages: usize,
    mode: PdfMode,
) -> Result<Vec<usize>, String> {
    let Some(value) = value else {
        return match mode {
            PdfMode::Text if total_pages > DEFAULT_MAX_PAGES => Err(format!(
                "This PDF has {total_pages} pages. Specify the pages parameter (e.g. \"1-10\"); max 20 pages per call."
            )),
            PdfMode::Text => Ok((1..=total_pages).collect()),
            PdfMode::Image => Ok((1..=total_pages.min(DEFAULT_IMAGE_PAGES)).collect()),
        };
    };
    let invalid = || {
        format!(
            "Invalid pages value \"{value}\". Use forms like \"3\", \"1-5\" (max 20 pages per call)."
        )
    };
    let (start, end) = if let Some((start, end)) = value.split_once('-') {
        if end.contains('-') {
            return Err(invalid());
        }
        let start = start.parse::<usize>().map_err(|_| invalid())?;
        let end = end.parse::<usize>().map_err(|_| invalid())?;
        if start > end {
            return Err(invalid());
        }
        (start, end)
    } else {
        let page = value.parse::<usize>().map_err(|_| invalid())?;
        (page, page)
    };
    if start == 0 || end > total_pages {
        return Err(format!(
            "Page range \"{value}\" is out of bounds: this PDF has {total_pages} pages."
        ));
    }
    if end - start + 1 > MAX_PAGES_PER_CALL {
        return Err(invalid());
    }
    Ok((start..=end).collect())
}

fn pdf_load_error(error: PdfiumError) -> ToolResponse {
    match error {
        PdfiumError::PdfiumLibraryInternalError(
            PdfiumInternalError::PasswordError | PdfiumInternalError::SecurityError,
        ) => ToolResponse::error("Cannot read PDF: the file is password-protected."),
        _ => ToolResponse::error("Cannot read PDF: the file is corrupted or not a valid PDF."),
    }
}

fn corrupted_pdf() -> ToolResponse {
    ToolResponse::error("Cannot read PDF: the file is corrupted or not a valid PDF.")
}

fn pdf_engine_error(reason: &str) -> ToolResponse {
    ToolResponse::error(format!(
        "PDF support is unavailable: could not load the bundled PDF engine ({reason}). Other file types are unaffected."
    ))
}

fn budget_too_small(budget: TokenBudget) -> ToolResponse {
    ToolResponse::error(format!(
        "{}={} is too small to return the required continuation note. Increase it and retry.",
        budget.variable, budget.value
    ))
}

fn normalize_pdf_text(text: &str) -> String {
    text.replace("\r\n", "\n")
        .replace('\r', "\n")
        .trim_end_matches('\n')
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::{
        PdfMode, RenderPlan, TextPage, collect_encoded_images, format_text_pages, parse_pages,
        parse_pdf_mode, pdf_engine_error, pdf_terminal_note, render_dimensions,
    };
    use crate::budget::TokenBudget;
    use crate::{ToolContent, ToolResponse};

    #[test]
    fn parses_single_and_range_pages() {
        assert_eq!(parse_pages(Some("3"), 5, PdfMode::Text).unwrap(), vec![3]);
        assert_eq!(
            parse_pages(Some("2-4"), 5, PdfMode::Image).unwrap(),
            vec![2, 3, 4]
        );
        assert_eq!(
            parse_pages(Some("1-20"), 20, PdfMode::Image).unwrap(),
            (1..=20).collect::<Vec<_>>()
        );
    }

    #[test]
    fn rejects_large_or_invalid_ranges() {
        assert!(
            parse_pages(Some("4-2"), 5, PdfMode::Text)
                .unwrap_err()
                .starts_with("Invalid pages")
        );
        assert!(
            parse_pages(Some("0"), 5, PdfMode::Text)
                .unwrap_err()
                .contains("out of bounds")
        );
        assert!(
            parse_pages(Some("1-21"), 21, PdfMode::Image)
                .unwrap_err()
                .starts_with("Invalid pages")
        );
    }

    #[test]
    fn default_page_selection_depends_on_pdf_mode() {
        assert_eq!(
            parse_pages(None, 5, PdfMode::Text).unwrap(),
            vec![1, 2, 3, 4, 5]
        );
        assert_eq!(
            parse_pages(None, 5, PdfMode::Image).unwrap(),
            vec![1, 2, 3, 4]
        );
        assert!(parse_pages(None, 11, PdfMode::Text).is_err());
        assert_eq!(
            parse_pages(None, 11, PdfMode::Image).unwrap(),
            vec![1, 2, 3, 4]
        );
    }

    #[test]
    fn pdf_mode_defaults_to_text_and_rejects_unknown_values() {
        assert_eq!(parse_pdf_mode(None).unwrap(), PdfMode::Text);
        assert_eq!(parse_pdf_mode(Some("image")).unwrap(), PdfMode::Image);
        assert_eq!(
            parse_pdf_mode(Some("both")).unwrap_err(),
            "Invalid pdf_mode value \"both\". Use \"text\" or \"image\"."
        );
    }

    #[test]
    fn terminal_notes_keep_the_delivered_page_width_and_mode_verb() {
        assert_eq!(
            pdf_terminal_note(1, 1, 1, PdfMode::Text, 1),
            "(Complete: page 1 of 1 shown.)"
        );
        assert_eq!(
            pdf_terminal_note(2, 3, 25, PdfMode::Image, 2),
            "(Partial: pages 2-3 of 25 rendered. Continue with pages=\"4-5\".)"
        );
        assert_eq!(
            pdf_terminal_note(25, 25, 25, PdfMode::Image, 1),
            "(Complete: page 25 of 25 rendered.)"
        );
    }

    #[test]
    fn image_payload_stops_on_a_page_boundary_and_keeps_terminal_last() {
        let plans = [
            RenderPlan {
                number: 1,
                width: 1,
                height: 1,
            },
            RenderPlan {
                number: 2,
                width: 1,
                height: 1,
            },
            RenderPlan {
                number: 3,
                width: 1,
                height: 1,
            },
        ];
        let response = collect_encoded_images(&plans, 5, 8, |_| Ok("AAAA".to_string()));
        assert_eq!(response.content.len(), 3);
        assert!(matches!(response.content[0], ToolContent::Image { .. }));
        assert!(matches!(response.content[1], ToolContent::Image { .. }));
        assert_eq!(
            response.content[2],
            ToolContent::Text(
                "(Partial: pages 1-2 of 5 rendered. Continue with pages=\"3-4\".)".to_string()
            )
        );
    }

    #[test]
    fn text_budget_stops_before_the_next_whole_page() {
        let pages = [
            TextPage {
                number: 1,
                text: "Small".to_string(),
            },
            TextPage {
                number: 2,
                text: "x".repeat(5_000),
            },
        ];
        let response = format_text_pages(
            &pages,
            2,
            TokenBudget {
                value: 80,
                variable: "FASTCTX_READ_TOKEN_BUDGET",
            },
        );
        assert_eq!(
            response,
            ToolResponse::text(
                "=== Page 1 ===\nSmall\n\n(Partial: page 1 of 2 shown. Continue with pages=\"2\".)"
            )
        );
    }

    #[test]
    fn first_text_page_truncates_only_at_a_line_boundary() {
        let pages = [TextPage {
            number: 1,
            text: format!("keep this line\n{}", "x".repeat(5_000)),
        }];
        let response = format_text_pages(
            &pages,
            2,
            TokenBudget {
                value: 80,
                variable: "FASTCTX_READ_TOKEN_BUDGET",
            },
        );
        assert_eq!(
            response,
            ToolResponse::text(
                "=== Page 1 ===\nkeep this line\n\n(Note: page 1 text truncated at the token budget; use pdf_mode=\"image\" to view the full page.)\n(Partial: page 1 of 2 shown. Continue with pages=\"2\".)"
            )
        );
    }

    #[test]
    fn first_image_over_the_payload_limit_is_an_actionable_error() {
        let plans = [RenderPlan {
            number: 7,
            width: 1,
            height: 1,
        }];
        let response = collect_encoded_images(&plans, 10, 3, |_| Ok("AAAA".to_string()));
        assert_eq!(
            response,
            ToolResponse::error(
                "Cannot return PDF page 7 as an image: the encoded image exceeds the 8 MiB payload safety limit. Use pdf_mode=\"text\" for this page."
            )
        );
    }

    #[test]
    fn engine_failure_keeps_other_file_types_actionable() {
        let response = pdf_engine_error("binding failed");
        assert!(response.is_error);
        assert_eq!(
            response.content,
            vec![ToolContent::Text(
                "PDF support is unavailable: could not load the bundled PDF engine (binding failed). Other file types are unaffected."
                    .to_string()
            )]
        );
    }

    #[test]
    fn invalid_render_dimensions_have_an_exact_repair_error() {
        let response = render_dimensions(7, 0.0, 842.0).unwrap_err();
        assert_eq!(
            response.content,
            vec![ToolContent::Text(
                "Cannot render PDF page 7: invalid page dimensions. Repair or regenerate the PDF externally."
                    .to_string()
            )]
        );
        assert!(render_dimensions(7, f32::NAN, 842.0).is_err());
    }
}
