//! File text display and the shared writing rules.

pub(crate) use fastctx_types::write::text::{dominant_ending, line_content, newlines};

/// Controls are escaped so terminals cannot overwrite the displayed file text.
pub(crate) fn visible(text: &str) -> String {
    let mut shown = String::new();
    for c in text.chars() {
        if c.is_control() && c != '\t' {
            for byte in c.encode_utf8(&mut [0; 4]).bytes() {
                shown.push_str(&format!(r"\x{byte:02X}"));
            }
        } else {
            shown.push(c);
        }
    }
    shown
}

pub(crate) fn visible_length(text: &str) -> usize {
    text.chars().map(|c| if c.is_control() && c != '\t' { c.len_utf8() * 4 } else { 1 }).sum()
}
