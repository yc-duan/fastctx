//! Selected PDF pages, processed one at a time without a page-count ceiling.

use super::{
    Block, Range, ViewArgs, image::Picture, note, pdf_document::Document, text::TextWindow,
};
use crate::head_note::{CoverageTotal, CoveredRange, HeadMetric, HeadNote};
use std::fs::File;

pub(super) fn read(
    file: File,
    subject: &str,
    bytes: u64,
    args: &ViewArgs,
    budget: usize,
) -> Result<(Vec<Block>, bool), String> {
    let (_session, pdfium) = crate::read_tool::pdf_engine::pdfium_session()?;
    if args.image {
        return images(pdfium, file, subject, args, budget);
    }
    let pages =
        Document::open(pdfium, file.try_clone().map_err(|error| error.to_string())?)?.pages()?;
    if pages == 0 {
        return Ok((vec![Block::Text(note(subject, "0 pages"))], true));
    }
    let range = args.pages.unwrap_or(Range {
        first: 1,
        last: pages,
    });
    if range.first > pages || range.last > pages {
        return Err(format!(
            "Page range {}-{} is outside this PDF's {pages} pages.",
            range.first, range.last
        ));
    }
    let Some(budget) = budget.checked_sub(
        page_note(subject, range.first, range.last, pages)
            .chars()
            .count(),
    ) else {
        return Ok((Vec::new(), false));
    };
    let mut blocks = Vec::new();
    let mut remaining = budget;
    for number in range.first..=range.last {
        let page_subject = format!("{subject}, page {number} of {pages}");
        // Pdfium retains parsed image streams until the document closes, even
        // after a page closes. Reopen the same file handle per page to release
        // those resources instead of accumulating the entire PDF. (2026-10-02)
        let document =
            Document::open(pdfium, file.try_clone().map_err(|error| error.to_string())?)?;
        let page = document.page(number)?;
        let block = {
            let mut text = page.text()?;
            if text.is_empty() {
                Block::Text(note(&page_subject, "no text layer"))
            } else {
                let window = TextWindow::read(|| text.next_char(), args.lines, remaining, None)?;
                let Some((text, complete)) = window.fit(&page_subject, bytes, None, remaining)
                else {
                    return Ok(finish(blocks, false, subject, range.first, pages));
                };
                let block = Block::Text(text);
                if !complete {
                    blocks.push(block);
                    return Ok(finish(blocks, false, subject, range.first, pages));
                }
                block
            }
        };
        let cost = block.cost();
        if cost > remaining {
            return Ok(finish(blocks, false, subject, range.first, pages));
        }
        remaining -= cost;
        blocks.push(block);
    }
    Ok(finish(blocks, true, subject, range.first, pages))
}

fn images(
    pdfium: &pdfium_render::prelude::Pdfium,
    file: File,
    subject: &str,
    args: &ViewArgs,
    budget: usize,
) -> Result<(Vec<Block>, bool), String> {
    use pdfium_render::prelude::PdfRenderConfig;
    let pages = pdfium
        .load_pdf_from_reader(file.try_clone().map_err(|error| error.to_string())?, None)
        .map_err(|error| format!("Cannot read PDF: {error}"))?;
    let total_pages = pages.pages().len() as usize;
    drop(pages);
    let pages = total_pages;
    let range = args.pages.unwrap_or(Range {
        first: 1,
        last: pages,
    });
    if pages == 0 {
        return Ok((vec![Block::Text(note(subject, "0 pages"))], true));
    }
    if range.first > pages || range.last > pages {
        return Err(format!(
            "Page range {}-{} is outside this PDF's {pages} pages.",
            range.first, range.last
        ));
    }
    let Some(budget) = budget.checked_sub(
        page_note(subject, range.first, range.last, pages)
            .chars()
            .count(),
    ) else {
        return Ok((Vec::new(), false));
    };
    let mut pictures = Vec::new();
    let mut remaining = budget;
    for number in range.first..=range.last {
        let page_subject = format!("{subject}, page {number} of {pages}");
        if remaining < super::IMAGE_CHARACTERS + note(&page_subject, "image 1x1").chars().count() {
            return Ok(finish(
                pictures.into_iter().map(Block::Image).collect(),
                false,
                subject,
                range.first,
                pages,
            ));
        }
        // Keep Pdfium's document-scoped resource caches bounded to this page.
        let document = pdfium
            .load_pdf_from_reader(file.try_clone().map_err(|error| error.to_string())?, None)
            .map_err(|error| format!("Cannot read PDF: {error}"))?;
        let page = document
            .pages()
            .get((number - 1) as i32)
            .map_err(|error| format!("Cannot read PDF page: {error}"))?;
        // PDF points are 1/72 inch; use the same 150 DPI as inspect_local_file.
        let width = (page.width().value * 150.0 / 72.0).ceil();
        let height = (page.height().value * 150.0 / 72.0).ceil();
        if !width.is_finite()
            || !height.is_finite()
            || width <= 0.0
            || height <= 0.0
            || width as f64 > i32::MAX as f64
            || height as f64 > i32::MAX as f64
        {
            return Err("PDF page dimensions cannot be represented by the renderer.".to_string());
        }
        let rendered = page
            .render_with_config(
                &PdfRenderConfig::new().set_target_size(width as i32, height as i32),
            )
            .and_then(|bitmap| bitmap.as_image())
            .map_err(|error| format!("Cannot render PDF page: {error}"))?;
        let picture = Picture::rendered(&page_subject, rendered)?;
        if picture.cost() > remaining {
            return Ok(finish(
                pictures.into_iter().map(Block::Image).collect(),
                false,
                subject,
                range.first,
                pages,
            ));
        }
        let previous = pictures.clone();
        pictures.push(picture);
        // Enforce the same byte ceiling while collecting pages, so one PDF cannot
        // accumulate all of its rendered pages before the response-wide check.
        super::image::shrink(&mut pictures.iter_mut().collect::<Vec<_>>())
            .map_err(|(_, error)| error)?;
        let cost = pictures.iter().map(Picture::cost).sum();
        let Some(after) = budget.checked_sub(cost) else {
            return Ok(finish(
                previous.into_iter().map(Block::Image).collect(),
                false,
                subject,
                range.first,
                pages,
            ));
        };
        remaining = after;
    }
    Ok(finish(
        pictures.into_iter().map(Block::Image).collect(),
        true,
        subject,
        range.first,
        pages,
    ))
}

fn page_note(subject: &str, first: usize, last: usize, total: usize) -> String {
    format!(
        "{}\n",
        HeadNote::new(
            subject,
            HeadMetric::Coverage {
                unit: "pages",
                ranges: vec![CoveredRange::new(first, last)],
                total: CoverageTotal::Exact(total),
            }
        )
        .render()
    )
}

fn finish(
    mut blocks: Vec<Block>,
    complete: bool,
    subject: &str,
    first: usize,
    total: usize,
) -> (Vec<Block>, bool) {
    if !blocks.is_empty() {
        blocks.insert(
            0,
            Block::Text(page_note(subject, first, first + blocks.len() - 1, total)),
        );
    }
    (blocks, complete)
}
