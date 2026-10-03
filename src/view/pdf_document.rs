//! Small RAII wrappers around Pdfium's random-access file and character APIs.

use pdfium_render::prelude::*;
use std::cell::RefCell;
use std::ffi::{c_int, c_ulong, c_void};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

struct Source {
    file: File,
    error: Option<String>,
}

pub(super) struct Document<'a> {
    bindings: &'a dyn PdfiumLibraryBindings,
    handle: FPDF_DOCUMENT,
    source: Box<RefCell<Source>>,
    _access: Box<FPDF_FILEACCESS>,
}

unsafe extern "C" fn read_block(
    parameter: *mut c_void,
    position: c_ulong,
    buffer: *mut u8,
    size: c_ulong,
) -> c_int {
    // Pdfium calls this synchronously while Document owns both the source and access.
    let mut source = unsafe { &*parameter.cast::<RefCell<Source>>() }.borrow_mut();
    let bytes = unsafe { std::slice::from_raw_parts_mut(buffer, size as usize) };
    #[cfg(windows)]
    let offset = u64::from(position);
    #[cfg(not(windows))]
    let offset = position;
    match source
        .file
        .seek(SeekFrom::Start(offset))
        .and_then(|_| source.file.read_exact(bytes))
    {
        Ok(()) => 1,
        Err(error) => {
            source.error = Some(error.to_string());
            0
        }
    }
}

impl<'a> Document<'a> {
    pub(super) fn open(pdfium: &'a Pdfium, file: File) -> Result<Self, String> {
        let length = file.metadata().map_err(|error| error.to_string())?.len();
        // Pdfium uses a 32-bit unsigned long on Windows and a 64-bit one on
        // the supported Unix targets. Reject lengths the native ABI cannot hold. (2026-10-02)
        #[cfg(windows)]
        let file_length = length.try_into().map_err(|_| {
            "PDF exceeds the file length supported by this Pdfium platform.".to_string()
        })?;
        #[cfg(not(windows))]
        let file_length = length;
        let source = Box::new(RefCell::new(Source { file, error: None }));
        let mut access = Box::new(FPDF_FILEACCESS {
            m_FileLen: file_length,
            m_GetBlock: Some(read_block),
            m_Param: (&*source as *const RefCell<Source>).cast_mut().cast(),
        });
        let bindings = pdfium.bindings();
        let handle = unsafe { bindings.FPDF_LoadCustomDocument(&mut *access, None) };
        if handle.is_null() {
            return Err(source
                .borrow_mut()
                .error
                .take()
                .unwrap_or_else(|| pdf_error(bindings)));
        }
        Ok(Self {
            bindings,
            handle,
            source,
            _access: access,
        })
    }

    pub(super) fn pages(&self) -> Result<usize, String> {
        let count = unsafe { self.bindings.FPDF_GetPageCount(self.handle) };
        usize::try_from(count).map_err(|_| pdf_error(self.bindings))
    }

    pub(super) fn page(&self, number: usize) -> Result<Page<'_>, String> {
        let handle = unsafe {
            self.bindings
                .FPDF_LoadPage(self.handle, (number - 1) as i32)
        };
        if handle.is_null() {
            return Err(self
                .source
                .borrow()
                .error
                .clone()
                .unwrap_or_else(|| pdf_error(self.bindings)));
        }
        Ok(Page {
            bindings: self.bindings,
            handle,
        })
    }
}

impl Drop for Document<'_> {
    fn drop(&mut self) {
        unsafe {
            self.bindings.FPDF_CloseDocument(self.handle);
        }
    }
}

pub(super) struct Page<'a> {
    bindings: &'a dyn PdfiumLibraryBindings,
    handle: FPDF_PAGE,
}

impl Page<'_> {
    pub(super) fn text(&self) -> Result<Text<'_>, String> {
        let handle = unsafe { self.bindings.FPDFText_LoadPage(self.handle) };
        if handle.is_null() {
            return Err(pdf_error(self.bindings));
        }
        let length = unsafe { self.bindings.FPDFText_CountChars(handle) };
        if length < 0 {
            unsafe {
                self.bindings.FPDFText_ClosePage(handle);
            }
            return Err(pdf_error(self.bindings));
        }
        Ok(Text {
            bindings: self.bindings,
            handle,
            length,
            position: 0,
        })
    }
}

impl Drop for Page<'_> {
    fn drop(&mut self) {
        unsafe {
            self.bindings.FPDF_ClosePage(self.handle);
        }
    }
}

pub(super) struct Text<'a> {
    bindings: &'a dyn PdfiumLibraryBindings,
    handle: FPDF_TEXTPAGE,
    length: i32,
    position: i32,
}

impl Text<'_> {
    pub(super) fn is_empty(&self) -> bool {
        self.length == 0
    }
    pub(super) fn next_char(&mut self) -> Result<Option<char>, String> {
        if self.position == self.length {
            return Ok(None);
        }
        let value = unsafe {
            self.bindings
                .FPDFText_GetUnicode(self.handle, self.position)
        };
        self.position += 1;
        char::from_u32(value).filter(|_| value != 0).map(Some).ok_or_else(|| format!("Cannot decode PDF text at character {}: missing or invalid Unicode value {value}.", self.position))
    }
}

impl Drop for Text<'_> {
    fn drop(&mut self) {
        unsafe {
            self.bindings.FPDFText_ClosePage(self.handle);
        }
    }
}

fn pdf_error(bindings: &dyn PdfiumLibraryBindings) -> String {
    let code = unsafe { bindings.FPDF_GetLastError() };
    let reason = match code {
        2 => "cannot open file",
        3 => "invalid or corrupted PDF",
        4 => "password required",
        5 => "unsupported security scheme",
        6 => "cannot load page",
        _ => "PDF engine failure",
    };
    format!("Cannot read PDF: {reason} (error {code}).")
}
