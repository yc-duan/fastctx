//! PDF page selection, text extraction, and 150-DPI page rendering.

use crate::budget::{TokenBudget, estimate_tokens};
use crate::head_note::{CoverageTotal, CoveredRange, HeadMetric, HeadNote};
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
            &crate::paths::display_path(path),
            &document,
            &selected,
            total_pages,
            text_budget.expect("text mode always receives a token budget"),
        ),
        PdfMode::Image => read_pdf_images(
            &crate::paths::display_path(path),
            &document,
            &selected,
            total_pages,
        ),
    }
}

fn read_pdf_text(
    path_display: &str,
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
    format_text_pages(path_display, &pages, total_pages, budget)
}

fn format_text_pages(
    path_display: &str,
    pages: &[TextPage],
    total_pages: usize,
    budget: TokenBudget,
) -> ToolResponse {
    let selected_all_no_text = pages.iter().all(|page| page.text.trim().is_empty());
    for shown in (1..=pages.len()).rev() {
        let output = render_text_output(
            path_display,
            &pages[..shown],
            total_pages,
            selected_all_no_text,
            false,
        );
        if estimate_tokens(&output) <= budget.value {
            return ToolResponse::text(output);
        }
    }

    truncate_first_text_page(path_display, &pages[0], total_pages, budget)
}

fn truncate_first_text_page(
    path_display: &str,
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
        let output = render_text_output(path_display, &[truncated], total_pages, false, true);
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
    path_display: &str,
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
    let mut note = HeadNote::new(
        path_display,
        HeadMetric::Coverage {
            unit: "pages",
            ranges: vec![CoveredRange::new(
                pages[0].number,
                pages.last().expect("text response has a page").number,
            )],
            total: CoverageTotal::Exact(total_pages),
        },
    );
    if selected_all_no_text {
        note = note.fact("selected pages have no text layer");
    }
    if first_page_truncated {
        note = note.fact(format!(
            "page {} text cut at the FastCtx token budget",
            pages[0].number
        ));
    }
    note.render_with_body(&body)
}

fn text_page_block(page: &TextPage) -> String {
    if page.text.trim().is_empty() {
        format!("=== Page {} === (no text layer)", page.number)
    } else {
        format!("=== Page {} ===\n{}", page.number, page.text)
    }
}

fn read_pdf_images(
    path_display: &str,
    document: &PdfDocument<'_>,
    selected: &[usize],
    total_pages: usize,
) -> ToolResponse {
    let plans = match preflight_render_plans(document, selected) {
        Ok(plans) => plans,
        Err(response) => return response,
    };
    collect_encoded_images(
        path_display,
        &plans,
        total_pages,
        MAX_IMAGE_PAYLOAD_BYTES,
        |plan| encode_page_png(document, plan),
    )
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
    path_display: &str,
    plans: &[RenderPlan],
    total_pages: usize,
    payload_limit: usize,
    mut encode: impl FnMut(&RenderPlan) -> Result<String, ToolResponse>,
) -> ToolResponse {
    let mut images = Vec::with_capacity(plans.len());
    let mut payload_bytes = 0_usize;
    for plan in plans {
        let data = match encode(plan) {
            Ok(data) => data,
            Err(response) => return response,
        };
        if payload_bytes.saturating_add(data.len()) > payload_limit {
            if images.is_empty() {
                return ToolResponse::error(format!(
                    "Cannot return PDF page {} as an image: the encoded image exceeds the 8 MiB payload safety limit. Use pdf_mode=\"text\" for this page.",
                    plan.number
                ));
            }
            break;
        }
        payload_bytes = payload_bytes.saturating_add(data.len());
        images.push(ToolContent::Image {
            data,
            mime_type: "image/png".to_string(),
            detail: Some(ImageDetail::High),
        });
    }
    let delivered = images.len();
    let first = plans[0].number;
    let last = plans[delivered - 1].number;
    let note = HeadNote::new(
        path_display,
        HeadMetric::Coverage {
            unit: "pages",
            ranges: vec![CoveredRange::new(first, last)],
            total: CoverageTotal::Exact(total_pages),
        },
    );
    let mut content = Vec::with_capacity(images.len() + 1);
    content.push(ToolContent::Text(note.render()));
    content.extend(images);
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
        "{}={} is too small to return the response head note and PDF content. That budget is fixed for this session; retrying cannot raise it.",
        budget.variable, budget.value
    ))
}

fn normalize_pdf_text(text: &str) -> String {
    text.replace("\r\n", "\n")
        .replace('\r', "\n")
        .trim_end_matches('\n')
        .to_string()
}
