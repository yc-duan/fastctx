//! Text in the machine's legacy encoding.
//!
//! Programs on Windows that do not write UTF-8 write in the system's code
//! page: GBK on a Chinese system, for instance. Unix has no such fallback
//! that could be relied on.

/// Decodes `bytes` strictly in this machine's legacy code pages, trying the
/// ANSI page and then the OEM page that console programs use. Returns the
/// text and the encoding's name, or `None` if neither decodes every byte.
#[cfg(windows)]
pub fn decode_legacy(bytes: &[u8]) -> Option<(String, String)> {
    use windows_sys::Win32::Globalization::{GetACP, GetOEMCP, MB_ERR_INVALID_CHARS, MultiByteToWideChar};

    if bytes.is_empty() || bytes.len() > i32::MAX as usize {
        return None;
    }
    // SAFETY: plain queries.
    let pages = unsafe { [GetACP(), GetOEMCP()] };
    for (index, &page) in pages.iter().enumerate() {
        if index == 1 && page == pages[0] {
            break;
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
                continue;
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
                continue;
            }
            if let Ok(text) = String::from_utf16(&wide) {
                return Some((text, page_name(page)));
            }
        }
    }
    None
}

#[cfg(windows)]
fn page_name(page: u32) -> String {
    match page {
        936 => "GBK (code page 936)".to_string(),
        950 => "Big5 (code page 950)".to_string(),
        932 => "Shift_JIS (code page 932)".to_string(),
        949 => "EUC-KR (code page 949)".to_string(),
        other => format!("code page {other}"),
    }
}

#[cfg(unix)]
pub fn decode_legacy(_bytes: &[u8]) -> Option<(String, String)> {
    None
}
