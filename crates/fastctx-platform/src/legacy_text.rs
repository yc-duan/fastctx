//! Strict decoding in this machine's ANSI and OEM code pages.

/// A code page selected for a complete line. Its fragments must use the
/// same interpretation even when separated by terminal images.
#[derive(Clone, Copy)]
pub struct LegacyEncoding {
    #[cfg(windows)]
    page: u32,
    #[cfg(unix)]
    _private: (),
}

/// Decodes `bytes` strictly in this machine's legacy code pages, trying the
/// ANSI page and then the OEM page that console programs use. Returns the
/// text and the encoding's name, or `None` if neither decodes every byte.
pub fn decode_legacy(bytes: &[u8]) -> Option<(String, String)> {
    decode_with_encoding(bytes).map(|(text, encoding)| (text, encoding.name()))
}

/// Tries ANSI before OEM and also returns the decoder for that code page.
#[cfg(windows)]
pub fn decode_with_encoding(bytes: &[u8]) -> Option<(String, LegacyEncoding)> {
    use windows_sys::Win32::Globalization::{GetACP, GetOEMCP};

    if bytes.is_empty() || bytes.len() > i32::MAX as usize {
        return None;
    }
    // SAFETY: plain queries.
    let pages = unsafe { [GetACP(), GetOEMCP()] };
    for (index, &page) in pages.iter().enumerate() {
        if index == 1 && page == pages[0] {
            break;
        }
        let encoding = LegacyEncoding { page };
        if let Some(text) = encoding.decode(bytes) {
            return Some((text, encoding));
        }
    }
    None
}

impl LegacyEncoding {
    /// Decodes strictly without changing the selected code page.
    #[cfg(windows)]
    pub fn decode(self, bytes: &[u8]) -> Option<String> {
        use windows_sys::Win32::Globalization::{MB_ERR_INVALID_CHARS, MultiByteToWideChar};
        if bytes.is_empty() {
            return Some(String::new());
        }
        let size = i32::try_from(bytes.len()).ok()?;
        // SAFETY: a size query, then a conversion into a buffer of that size.
        unsafe {
            let length =
                MultiByteToWideChar(self.page, MB_ERR_INVALID_CHARS, bytes.as_ptr(), size, std::ptr::null_mut(), 0);
            if length <= 0 {
                return None;
            }
            let mut wide = vec![0u16; length as usize];
            if MultiByteToWideChar(self.page, MB_ERR_INVALID_CHARS, bytes.as_ptr(), size, wide.as_mut_ptr(), length)
                != length
            {
                return None;
            }
            String::from_utf16(&wide).ok()
        }
    }

    #[cfg(windows)]
    pub fn name(self) -> String {
        match self.page {
            936 => "GBK (code page 936)".to_string(),
            950 => "Big5 (code page 950)".to_string(),
            932 => "Shift_JIS (code page 932)".to_string(),
            949 => "EUC-KR (code page 949)".to_string(),
            other => format!("code page {other}"),
        }
    }

    #[cfg(unix)]
    pub fn decode(self, _bytes: &[u8]) -> Option<String> {
        None
    }

    #[cfg(unix)]
    pub fn name(self) -> String {
        unreachable!("Unix has no system legacy encoding")
    }
}

#[cfg(unix)]
pub fn decode_with_encoding(_bytes: &[u8]) -> Option<(String, LegacyEncoding)> {
    None
}
