//! File text rules shared by viewing and writing. A bare CR is data.

pub(crate) fn dominant_ending(text: &str) -> &'static str {
    let crlf = text.matches("\r\n").count();
    let lf = text.bytes().filter(|&b| b == b'\n').count() - crlf;
    if crlf > lf { "\r\n" } else { "\n" }
}

pub(crate) fn replacement_ending(text: &str, dominant: &'static str) -> &'static str {
    let crlf = text.matches("\r\n").count();
    let lf = text.bytes().filter(|&b| b == b'\n').count() - crlf;
    match (crlf, lf) {
        (0, 0) => dominant,
        (_, 0) => "\r\n",
        (0, _) => "\n",
        _ => dominant,
    }
}

pub(crate) fn newlines(text: &str, ending: &str) -> String {
    text.replace("\r\n", "\n").replace('\n', ending)
}

/// Removes only a line terminator, never a bare CR at EOF.
pub(crate) fn line_content(text: &str) -> &str {
    match text.strip_suffix('\n') {
        Some(text) => text.strip_suffix('\r').unwrap_or(text),
        None => text,
    }
}

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
