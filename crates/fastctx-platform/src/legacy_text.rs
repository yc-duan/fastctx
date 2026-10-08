//! Text in the machine's legacy encoding.
//!
//! Programs on Windows that do not write UTF-8 write in the system's code
//! page: GBK on a Chinese system, for instance. Unix has no such fallback
//! that could be relied on.

mod stream;
pub use stream::LegacyDecoder;

/// One native code page. The order returned by `code_pages` is significant.
#[derive(Clone, Copy)]
pub struct CodePage(u32);

impl CodePage {
    pub fn name(self) -> String {
        page_name(self.0)
    }

    /// A WHATWG label when the writing core can preserve this encoding.
    pub fn label(self) -> Option<&'static str> {
        match self.0 {
            65001 => Some("UTF-8"),
            936 => Some("GBK"),
            950 => Some("Big5"),
            932 => Some("Shift_JIS"),
            949 => Some("EUC-KR"),
            874 => Some("windows-874"),
            1250 => Some("windows-1250"),
            1251 => Some("windows-1251"),
            1252 => Some("windows-1252"),
            1253 => Some("windows-1253"),
            1254 => Some("windows-1254"),
            1255 => Some("windows-1255"),
            1256 => Some("windows-1256"),
            1257 => Some("windows-1257"),
            1258 => Some("windows-1258"),
            54936 => Some("GB18030"),
            _ => None,
        }
    }

    /// Uses the same strict native conversion as program output decoding.
    pub fn decode(self, bytes: &[u8]) -> Option<String> {
        decode_page(self.0, bytes)
    }
}

/// ANSI first, then OEM if distinct. Unix has no implicit legacy encoding.
pub fn code_pages() -> Vec<CodePage> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Globalization::{GetACP, GetOEMCP};
        // SAFETY: plain queries.
        let [ansi, oem] = unsafe { [GetACP(), GetOEMCP()] };
        if ansi == oem { vec![CodePage(ansi)] } else { vec![CodePage(ansi), CodePage(oem)] }
    }
    #[cfg(not(windows))]
    Vec::new()
}

/// Decodes `bytes` strictly in this machine's legacy code pages, trying the
/// ANSI page and then the OEM page that console programs use. Returns the
/// text and the encoding's name, or `None` if neither decodes every byte.
pub fn decode_legacy(bytes: &[u8]) -> Option<(String, String)> {
    decode_with_code_page(bytes).map(|(text, page)| (text, page.name()))
}

/// `decode_legacy`, with the code page that decoded, so that the rest of a
/// line split around something else is decoded in the same one.
pub fn decode_with_code_page(bytes: &[u8]) -> Option<(String, CodePage)> {
    if bytes.is_empty() {
        return None;
    }
    code_pages().into_iter().find_map(|page| page.decode(bytes).map(|text| (text, page)))
}

#[cfg(windows)]
fn decode_page(page: u32, bytes: &[u8]) -> Option<String> {
    use windows_sys::Win32::Globalization::{MB_ERR_INVALID_CHARS, MultiByteToWideChar};

    if bytes.is_empty() {
        return Some(String::new());
    }
    if bytes.len() > i32::MAX as usize {
        return None;
    }
    // SAFETY: a size query, then a conversion into a buffer of that size.
    unsafe {
        let length = MultiByteToWideChar(
            page,
            MB_ERR_INVALID_CHARS,
            bytes.as_ptr(),
            bytes.len() as i32,
            std::ptr::null_mut(),
            0,
        );
        if length <= 0 {
            return None;
        }
        let mut wide = vec![0u16; length as usize];
        if MultiByteToWideChar(
            page,
            MB_ERR_INVALID_CHARS,
            bytes.as_ptr(),
            bytes.len() as i32,
            wide.as_mut_ptr(),
            length,
        ) != length
        {
            return None;
        }
        if let Ok(text) = String::from_utf16(&wide) {
            return Some(text);
        }
    }
    None
}

fn page_name(page: u32) -> String {
    match page {
        936 => "GBK (code page 936)".to_string(),
        950 => "Big5 (code page 950)".to_string(),
        932 => "Shift_JIS (code page 932)".to_string(),
        949 => "EUC-KR (code page 949)".to_string(),
        other => format!("code page {other}"),
    }
}

#[cfg(not(windows))]
fn decode_page(_page: u32, _bytes: &[u8]) -> Option<String> {
    None
}
