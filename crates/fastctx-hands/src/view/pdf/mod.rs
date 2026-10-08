//! PDF pages share the viewer's character and image budgets.

mod page_tree;
mod text;
mod validate;

use std::fs::{File, Metadata};
use std::io;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use fastctx_types::residents::Seen;
use hayro::hayro_interpret::font::FontQuery;
use hayro::hayro_interpret::{InterpreterSettings, InterpreterWarning};
use hayro::hayro_syntax::{DecryptionError, LoadPdfError};
use hayro::hayro_syntax::{Pdf, PdfData};
use jiff::Timestamp;

use super::{Options, STANDARD, Section};
use crate::run::window::IMAGE_COST;

pub(super) fn look(
    path: &Path,
    options: &Options,
    budget: usize,
    file: File,
    metadata: &Metadata,
) -> io::Result<(Section, Option<Seen>)> {
    if options.lines.is_some() || options.encoding.is_some() {
        return Ok((
            Section::note(path, options, "a PDF; --lines and --encoding apply to text files; use --pages"),
            None,
        ));
    }
    // Keep the file open while mapped. The mapping faults in only the xref,
    // page tree and selected page resources, rather than copying the whole PDF.
    // Like any file-backed mapping, the source must not be truncated while read.
    if metadata.len() == 0 {
        return Ok((Section::note(path, options, "invalid PDF: empty file"), None));
    }
    let mapping = match unsafe { memmap2::MmapOptions::new().map(&file) } {
        Ok(mapping) => mapping,
        Err(error) => return Ok((Section::note(path, options, &super::describe_error(&error)), None)),
    };
    let data = PdfData::from(Arc::new(mapping));
    let section = match catch_unwind(AssertUnwindSafe(|| read(path, options, budget, data))) {
        Ok(Ok(section)) => section,
        Ok(Err(reason)) => Section::note(path, options, &reason),
        Err(_) => Section::note(path, options, "PDF processing failed; the PDF engine rejected this file"),
    };
    let seen = section.shown.then(|| Seen {
        path: path.to_string_lossy().into_owned(),
        at: Timestamp::now(),
        // Extracted text and rasterized pages are views of the file, not its
        // complete bytes; hashing would require reading all unselected data.
        sha256: None,
        size: metadata.len(),
        modified: metadata.modified().ok().and_then(|time| Timestamp::try_from(time).ok()),
        encoding: None,
        encoding_selection: None,
    });
    Ok((section, seen))
}

fn open(data: PdfData) -> Result<Pdf, String> {
    let pdf = Pdf::new(data).map_err(|error| -> String {
        match error {
            LoadPdfError::Invalid => "invalid or damaged PDF; could not read its page tree".into(),
            LoadPdfError::Decryption(DecryptionError::PasswordProtected) => {
                "encrypted PDF; a password is required".into()
            }
            LoadPdfError::Decryption(DecryptionError::UnsupportedAlgorithm) => {
                "encrypted PDF; its encryption algorithm is unsupported".into()
            }
            LoadPdfError::Decryption(_) => "damaged PDF encryption information".into(),
        }
    })?;
    page_tree::check(&pdf)?;
    Ok(pdf)
}

fn read(path: &Path, options: &Options, budget: usize, data: PdfData) -> Result<Section, String> {
    let mut pdf = open(data.clone())?;
    let total = pdf.pages().len() as u64;
    let pages = super::plural(total, "page", "pages");
    let (first, asked_last) = options.pages.unwrap_or((1, total));
    if first == 0 || asked_last < first || first > total {
        return Ok(Section::note(path, options, &format!("PDF, {pages}; no pages in the range asked")));
    }
    let last = asked_last.min(total);
    let mut section = Section::note(path, options, &format!("PDF, {pages}"));
    // Reserve the longest possible stopping location before reading a page.
    // All page, line and character counts are usize/u64, at most 20 digits.
    let tail = format!(
        "=== page {n} (line {n}: characters 1-{n} of {n} shown; remaining selected content not shown) ===\n",
        n = u64::MAX
    );
    if section.cost + tail.chars().count() > budget {
        // The outer viewer has already reserved this exact file note. Even
        // the PDF header plus its stopping location may no longer fit.
        let mut note = super::not_shown(path, options);
        note.partial = true;
        return Ok(note);
    }
    let available = budget - section.cost - tail.chars().count();
    let mut left = available;
    for number in first..=last {
        if options.image && left < IMAGE_COST || left == 0 {
            section.append_note(&format!("=== pages {number}-{last} (not shown) ===\n"));
            section.partial = true;
            break;
        }
        // Hayro retains decoded page streams in the document. Reopening over
        // the same mapping releases the previous page and its resource caches.
        if number > first {
            pdf = open(data.clone())?;
        }
        let page = &pdf.pages()[(number - 1) as usize];
        let warnings = Arc::new(Mutex::new(Vec::new()));
        let sink = warnings.clone();
        let font_sink = warnings.clone();
        let default_fonts = InterpreterSettings::default().font_resolver;
        let settings = InterpreterSettings {
            font_resolver: Arc::new(move |query| {
                if matches!(query, FontQuery::Fallback(_)) {
                    let mut warnings = font_sink.lock().unwrap();
                    let reason = "a non-embedded font was substituted";
                    if !warnings.contains(&reason) {
                        warnings.push(reason);
                    }
                }
                default_fonts(query)
            }),
            warning_sink: Arc::new(move |warning| {
                let reason = match warning {
                    InterpreterWarning::UnsupportedFont => "unsupported font",
                    InterpreterWarning::ImageDecodeFailure => "an image could not be decoded",
                    InterpreterWarning::UnresolvedAnnotationAppearance => "an annotation could not be drawn",
                    _ => "unsupported PDF content",
                };
                let mut warnings = sink.lock().unwrap();
                if !warnings.contains(&reason) {
                    warnings.push(reason);
                }
            }),
            ..Default::default()
        };
        let warning_reserve = format!("=== page {} (a non-embedded font was substituted; unsupported font; an image could not be decoded; an annotation could not be drawn; unsupported PDF content; page content may be missing) ===\n", u64::MAX).len();
        let page_budget = left.saturating_sub(warning_reserve);
        if page_budget == 0 {
            section.append_note(&format!("=== pages {number}-{last} (not shown) ===\n"));
            section.partial = true;
            break;
        }
        let result = catch_unwind(AssertUnwindSafe(|| {
            validate::contents(page)?;
            if options.image {
                render(page, &settings, number, page_budget)
            } else {
                text::read(page, &settings, number, page_budget)
            }
        }));
        let part = match result {
            Ok(Ok(part)) => part,
            Ok(Err(reason)) => PageOutput::note(number, &reason),
            Err(_) => PageOutput::note(number, "PDF engine failed to process this page"),
        };
        let issues = warnings.lock().unwrap();
        let issue_note = if issues.is_empty() {
            String::new()
        } else {
            format!("=== page {number} ({}; page content may be missing) ===\n", issues.join("; "))
        };
        let cost = part.cost + issue_note.chars().count();
        if cost > left || part.text.is_empty() {
            section.append_note(&format!("=== pages {number}-{last} (not shown) ===\n"));
            section.partial = true;
            break;
        }
        section.text.push_str(&part.text);
        section.text.push_str(&issue_note);
        section.cost += cost;
        left -= cost;
        section.shown |= part.shown;
        if part.partial {
            if number < last {
                section.append_note(&format!("=== pages {}-{last} (not shown) ===\n", number + 1));
            }
            section.partial = true;
            break;
        }
    }
    Ok(section)
}

impl Section {
    fn append_note(&mut self, note: &str) {
        self.cost += note.chars().count();
        self.text.push_str(note);
    }
}

pub(super) struct PageOutput {
    text: String,
    cost: usize,
    shown: bool,
    partial: bool,
}

impl PageOutput {
    fn note(number: u64, reason: &str) -> Self {
        let text = format!("=== page {number} ({reason}) ===\n");
        Self { cost: text.chars().count(), text, shown: false, partial: false }
    }
}

fn render(
    page: &hayro::hayro_syntax::page::Page<'_>,
    settings: &InterpreterSettings,
    number: u64,
    budget: usize,
) -> Result<PageOutput, String> {
    let (width, height) = page.render_dimensions();
    if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 {
        return Err("invalid PDF page dimensions".into());
    }
    // Hosts reduce the long edge to about 2000 px. Rendering at that size
    // preserves all detail they can deliver without allocating a larger page.
    let scale = 2000.0 / width.max(height);
    if (width * scale) < 1.0 || (height * scale) < 1.0 {
        return Err("page aspect ratio gives less than one pixel on its short edge at 2000 pixels".into());
    }
    let note = format!("=== page {number} (image {}x{}) ===\n", (width * scale) as u16, (height * scale) as u16);
    let cost = note.chars().count() + IMAGE_COST + 1;
    if cost > budget {
        return Ok(PageOutput { text: String::new(), cost, shown: false, partial: true });
    }
    let pixmap = hayro::render(
        page,
        &hayro::RenderCache::new(),
        settings,
        &hayro::RenderSettings::default(),
        &hayro::PixmapSettings {
            x_scale: scale,
            y_scale: scale,
            bg_color: hayro::vello_cpu::color::palette::css::WHITE,
        },
    );
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, u32::from(pixmap.width()), u32::from(pixmap.height()));
        // White compositing makes alpha redundant. RGB also bounds all PDF
        // images below the 400 MiB transport budget: at most 28 images fit
        // 150000 / 5200, each at most 2000 * 2000 * 3 bytes before compression.
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
        let rgb: Vec<u8> =
            pixmap.data_as_u8_slice().chunks_exact(4).flat_map(|pixel| pixel[..3].iter().copied()).collect();
        writer.write_image_data(&rgb).map_err(|e| e.to_string())?;
    }
    let text = format!("{note}\x1b]1337;File=inline=1;size={}:{}\x07\n", bytes.len(), STANDARD.encode(bytes));
    Ok(PageOutput { text, cost, shown: true, partial: false })
}
