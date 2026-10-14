//! Office content, with document locations and an explicit output budget.
//! This crate performs no rendering, subprocess execution, or file writes.

mod arrays;
mod biff;
mod biff_string;
mod explore;
mod formula;
mod notes;
mod number;
mod output;
mod package;
mod slides;
mod spreadsheet;
mod word;
mod word_outline;
mod xls;
mod xls_explore;
mod xlsx;
mod xlsx_formula;
mod xstring;

use std::fs::File;
use std::path::Path;

use anyhow::{Result, bail};
pub use office_oxide::DocumentFormat as Format;
pub use spreadsheet::CellRange;

/// Selections count lines, slides and sheets from one. Sheet names are
/// literal; a separate index keeps numeric sheet names unambiguous.
#[derive(Debug, Default, Clone)]
pub struct Selection {
    pub lines: Option<(u64, u64)>,
    pub pages: Option<(u64, u64)>,
    pub sheet: Option<String>,
    pub sheet_index: Option<usize>,
    pub range: Option<CellRange>,
    /// Select only the presentation's master and layout text.
    pub master: bool,
}

/// A bounded view. `stopped` means the budget ended selected content;
/// explicit selections alone do not set it. Parse failures are errors.
pub struct Content {
    /// Facts for the caller's single file head note; counted in the budget.
    pub facts: String,
    pub text: String,
    pub stopped: bool,
    /// Whether all of the content is shown.
    pub whole: bool,
}

/// The six supported extensions, case insensitive.
pub fn format(path: &Path) -> Option<Format> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "docx" => Some(Format::Docx),
        "doc" => Some(Format::Doc),
        "pptx" => Some(Format::Pptx),
        "ppt" => Some(Format::Ppt),
        "xlsx" => Some(Format::Xlsx),
        "xls" => Some(Format::Xls),
        _ => None,
    }
}

/// Read from the already opened file. The budget, in UTF-8 bytes, includes every byte
/// in both facts and text, including stopping locations and continuation hints.
pub fn read(file: File, format: Format, selection: &Selection, budget: usize) -> Result<Content> {
    read_inner(file, format, selection, budget, None)
}

/// Explore content without a selection. Small documents are returned whole;
/// larger documents include a bounded beginning and format-specific locations.
/// `whole_budget` includes facts and text; the caller reserves its file header.
pub fn explore(file: File, format: Format, budget: usize, whole_budget: usize) -> Result<Content> {
    read_inner(file, format, &Selection::default(), budget, Some(whole_budget))
}

fn read_inner(
    file: File,
    format: Format,
    selection: &Selection,
    budget: usize,
    exploring: Option<usize>,
) -> Result<Content> {
    if selection.master && !matches!(format, Format::Ppt | Format::Pptx) {
        bail!("--master applies to PowerPoint presentations");
    }
    match format {
        Format::Xlsx | Format::Xls => {
            if selection.lines.is_some() || selection.pages.is_some() {
                bail!("spreadsheets use --sheet and --range, not --lines or --pages");
            }
        }
        _ if selection.sheet.is_some() || selection.sheet_index.is_some() || selection.range.is_some() => {
            bail!("--sheet and --range apply to Excel workbooks");
        }
        Format::Doc | Format::Docx if selection.pages.is_some() => {
            bail!("Word content uses --lines; page layout is not rendered");
        }
        _ => {}
    }
    if format == Format::Xlsx {
        return xlsx::read(file, selection, budget, exploring);
    }
    let formulas = if format == Format::Xls { Some(file.try_clone()?) } else { None };
    let notes = if format == Format::Pptx { Some(file.try_clone()?) } else { None };
    let document = office_oxide::Document::from_reader(file, format)?;
    if let Some(whole) = exploring {
        return match format {
            Format::Doc | Format::Docx => word::explore(&document, budget, whole),
            Format::Ppt | Format::Pptx => slides::explore(&document, notes, budget, whole),
            Format::Xls => xls_explore::explore(&document, formulas.unwrap(), budget, whole),
            _ => unreachable!(),
        };
    }
    let mut out = output::Output::new(budget);
    match format {
        Format::Doc | Format::Docx => word::read(&document, selection, &mut out)?,
        Format::Ppt | Format::Pptx => slides::read(&document, notes, selection, &mut out)?,
        Format::Xls => xls::read(&document, formulas.unwrap(), selection, &mut out)?,
        _ => unreachable!(),
    }
    Ok(out.finish())
}
