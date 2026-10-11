//! Extract positioned text and render individual PDF pages without external programs.

mod bidi;
mod fonts;
mod layout;
mod marks;
mod page_tree;
mod paragraphs;
mod pen;
mod reader;

mod text;
mod validate;

use std::fs::File;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::sync::{Arc, Mutex};

use hayro::hayro_interpret::{InterpreterSettings, InterpreterWarning};
use hayro::hayro_syntax::{DecryptionError, LoadPdfError, Pdf, PdfData};

/// A mapped document. Page indices in this API are zero-based.
///
/// The source file must not be truncated while this document is open.
pub struct Document {
    data: PdfData,
    syntax: Pdf,
    text_reader: reader::TextReader,
    pages: usize,
    path: std::path::PathBuf,
    fonts: fonts::Database,
}

/// Reading-order text and facts about text that could not be extracted.
pub struct TextPage {
    pub lines: Vec<String>,
    pub notes: Vec<String>,
}

/// A white-composited PNG at the requested size.
pub struct ImagePage {
    pub png: Vec<u8>,
    pub width: u16,
    pub height: u16,
    pub notes: Vec<String>,
}

impl Document {
    /// Open a regular PDF file. Password-required and damaged files are errors.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, String> {
        let path = path.as_ref().to_path_buf();
        // Opening a FIFO would wait for a writer before its metadata could be
        // checked. Reject non-files before opening, then inspect the handle.
        if !std::fs::metadata(&path).map_err(|error| error.to_string())?.is_file() {
            return Err("not a regular file".into());
        }
        let file = File::open(&path).map_err(|error| error.to_string())?;
        let metadata = file.metadata().map_err(|error| error.to_string())?;
        if !metadata.is_file() {
            return Err("not a regular file".into());
        }
        if metadata.len() == 0 {
            return Err("invalid PDF: empty file".into());
        }
        // Page streams and font programs are faulted in on demand. The caller
        // must keep the source from being truncated during this mapping.
        let mapping = unsafe { memmap2::MmapOptions::new().map(&file) }.map_err(|error| error.to_string())?;
        let data = PdfData::from(Arc::new(mapping));
        engine(|| {
            let pdf = open(data.clone())?;
            page_tree::check(&pdf)?;
            Ok(Self {
                data,
                pages: pdf.pages().len(),
                syntax: pdf,
                text_reader: reader::TextReader::default(),
                path,
                fonts: fonts::Database::new(),
            })
        })
    }

    pub fn page_count(&self) -> usize {
        self.pages
    }

    /// Visit document bookmarks in order. Pages are zero-based; a missing
    /// destination is `None`. Bookmark parsing failures are returned.
    /// Callbacks hold the text-reader lock and must not re-enter this document.
    pub fn outline(&self, visit: impl FnMut(&str, Option<usize>, usize)) -> Result<(), String> {
        engine(|| self.text_reader.outline(&self.path, visit))
    }

    /// Extract one page, including invisible OCR text. This does not perform OCR.
    /// Calls on the same document share parsed fonts and serialize extraction;
    /// decoded page content is released before returning.
    pub fn text(&self, index: usize) -> Result<TextPage, String> {
        engine(|| {
            let page = self
                .syntax
                .pages()
                .get(index)
                .ok_or_else(|| format!("page index {index} is outside {} pages", self.pages))?;
            validate::contents(page, |_| {})?;
            self.text_reader.read(&self.path, index, page)
        })
    }

    /// Render one page using embedded fonts and locally available substitutes.
    pub fn render(&self, index: usize, long_edge: u16) -> Result<ImagePage, String> {
        engine(|| {
            let pdf = self.page_document(index)?;
            let page = &pdf.pages()[index];
            validate::contents(page, |_| {})?;
            let (settings, warnings) = settings(true);
            let (settings, font_notes) = fonts::prepare(page, &settings, &self.fonts);
            let mut result = render(page, &settings, long_edge)?;
            result.notes.extend(font_notes);
            result.notes.extend(warnings.lock().unwrap().iter().map(|note| (*note).to_owned()));
            Ok(result)
        })
    }

    fn page_document(&self, index: usize) -> Result<Pdf, String> {
        if index >= self.pages {
            return Err(format!("page index {index} is outside {} pages", self.pages));
        }
        // A fresh interpreter document releases decoded streams between pages.
        open(self.data.clone())
    }
}

fn engine<T>(operation: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    catch_unwind(AssertUnwindSafe(operation))
        .unwrap_or_else(|_| Err("PDF engine failed to process this content".into()))
}

fn open(data: PdfData) -> Result<Pdf, String> {
    Pdf::new(data).map_err(|error| match error {
        LoadPdfError::Invalid => "invalid or damaged PDF; could not read its page tree".into(),
        LoadPdfError::Decryption(DecryptionError::PasswordProtected) => "encrypted PDF; a password is required".into(),
        LoadPdfError::Decryption(DecryptionError::UnsupportedAlgorithm) => {
            "encrypted PDF; its encryption algorithm is unsupported".into()
        }
        LoadPdfError::Decryption(_) => "damaged PDF encryption information".into(),
    })
}

type Warnings = Arc<Mutex<Vec<&'static str>>>;

fn settings(image: bool) -> (InterpreterSettings, Warnings) {
    let warnings = Warnings::default();
    let sink = warnings.clone();
    let settings = InterpreterSettings {
        warning_sink: Arc::new(move |warning| {
            let reason = match warning {
                InterpreterWarning::UnsupportedFont => "text uses an unsupported font",
                InterpreterWarning::ImageDecodeFailure if image => "an image could not be decoded",
                InterpreterWarning::UnresolvedAnnotationAppearance if image => "an annotation could not be drawn",
                InterpreterWarning::ImageDecodeFailure | InterpreterWarning::UnresolvedAnnotationAppearance => return,
                _ => "unsupported PDF content",
            };
            let mut warnings = sink.lock().unwrap();
            if !warnings.contains(&reason) {
                warnings.push(reason);
            }
        }),
        ..Default::default()
    };
    (settings, warnings)
}

fn render(
    page: &hayro::hayro_syntax::page::Page<'_>,
    settings: &InterpreterSettings,
    long_edge: u16,
) -> Result<ImagePage, String> {
    let (width, height) = page.render_dimensions();
    if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 {
        return Err("invalid PDF page dimensions".into());
    }
    if long_edge == 0 {
        return Err("render size must be greater than zero".into());
    }
    let scale = f32::from(long_edge) / width.max(height);
    if width * scale < 1.0 || height * scale < 1.0 {
        return Err(format!("page aspect ratio gives less than one pixel on its short edge at {long_edge} pixels"));
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
    let mut png = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png, u32::from(pixmap.width()), u32::from(pixmap.height()));
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(|error| error.to_string())?;
        let rgb: Vec<u8> =
            pixmap.data_as_u8_slice().chunks_exact(4).flat_map(|pixel| pixel[..3].iter().copied()).collect();
        writer.write_image_data(&rgb).map_err(|error| error.to_string())?;
    }
    Ok(ImagePage { png, width: pixmap.width(), height: pixmap.height(), notes: Vec::new() })
}
