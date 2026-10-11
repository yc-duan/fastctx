//! Reuse parsed fonts and document metadata without keeping decoded pages.

use hayro::hayro_syntax::page::Page;
use pdf_oxide::PdfDocument;
use std::path::Path;
use std::sync::Mutex;

#[derive(Default)]
pub(super) struct TextReader {
    document: Mutex<Option<PdfDocument>>,
}

impl TextReader {
    pub(super) fn outline(&self, path: &Path, mut visit: impl FnMut(&str, Option<usize>, usize)) -> Result<(), String> {
        let mut document = self.document.lock().expect("PDF reader lock");
        let result = super::engine(|| {
            if document.is_none() {
                *document = Some(PdfDocument::open_readonly(path).map_err(|error| error.to_string())?);
            }
            document
                .as_ref()
                .unwrap()
                .visit_outline(|title, page, level| {
                    visit(title, page, level);
                    Ok(())
                })
                .map_err(|error| error.to_string())
        });
        if result.is_err() {
            *document = None;
        }
        result
    }

    pub(super) fn read(&self, path: &Path, index: usize, page: &Page<'_>) -> Result<super::TextPage, String> {
        let mut document = self.document.lock().expect("page panics are caught while holding the reader lock");
        let result = super::engine(|| {
            if document.is_none() {
                *document = Some(PdfDocument::open_readonly(path).map_err(|error| error.to_string())?);
            }
            let document = document.as_ref().unwrap();
            let result = super::text::read(document, index, page);
            document.release_page_data();
            result
        });
        if result.is_err() {
            // A failed extraction may leave partially populated caches. The next
            // selected page must start from the source, as an independent read.
            *document = None;
        }
        result
    }
}
