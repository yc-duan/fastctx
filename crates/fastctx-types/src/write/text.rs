//! File text rules shared by viewing and writing. A bare CR is data.

pub fn dominant_ending(text: &str) -> &'static str {
    let crlf = text.matches("\r\n").count();
    let lf = text.bytes().filter(|&b| b == b'\n').count() - crlf;
    if crlf > lf { "\r\n" } else { "\n" }
}

pub fn replacement_ending(text: &str, dominant: &'static str) -> &'static str {
    let crlf = text.matches("\r\n").count();
    let lf = text.bytes().filter(|&b| b == b'\n').count() - crlf;
    match (crlf, lf) {
        (0, 0) => dominant,
        (_, 0) => "\r\n",
        (0, _) => "\n",
        _ => dominant,
    }
}

pub fn newlines(text: &str, ending: &str) -> String {
    text.replace("\r\n", "\n").replace('\n', ending)
}

/// Removes only a line terminator, never a bare CR at EOF.
pub fn line_content(text: &str) -> &str {
    match text.strip_suffix('\n') {
        Some(text) => text.strip_suffix('\r').unwrap_or(text),
        None => text,
    }
}
