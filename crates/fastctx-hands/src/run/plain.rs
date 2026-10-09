//! Rows whose only terminal instructions are colour changes need no cell layout.

use unicode_width::UnicodeWidthChar;

pub(super) enum Row {
    Original(usize),
    Rendered(String),
}

/// The caller supplies at most one input block. A rendered row that would
/// exceed the spool's in-memory allowance uses the general layout instead.
pub(super) fn text(bytes: &[u8]) -> Option<Row> {
    let text = std::str::from_utf8(bytes).ok()?;
    let carriage_return = text.ends_with('\r');
    let text = text.strip_suffix('\r').unwrap_or(text);
    let bytes = text.as_bytes();
    let mut output: Option<String> = None;
    let mut start = 0;
    let mut at = 0;
    while at < bytes.len() {
        match bytes[at] {
            b'\t' | 0x20..=0x7e => at += 1,
            0x1b => {
                if bytes.get(at + 1) != Some(&b'[') {
                    return None;
                }
                let mut end = at + 2;
                while bytes.get(end).is_some_and(|b| b.is_ascii_digit() || matches!(b, b';' | b':')) {
                    end += 1;
                }
                if bytes.get(end) != Some(&b'm') {
                    return None;
                }
                output.get_or_insert_with(|| String::with_capacity(text.len())).push_str(&text[start..at]);
                at = end + 1;
                start = at;
            }
            0x80..=0xff => {
                let c = text[at..].chars().next().expect("UTF-8 boundary");
                if c.is_control() {
                    return None;
                }
                at += c.len_utf8();
            }
            _ => return None,
        }
    }
    if let Some(output) = &mut output {
        output.push_str(&text[start..]);
    }
    if carriage_return && text.contains('\t') {
        return expand_tabs(output.as_deref().unwrap_or(text)).map(Row::Rendered);
    }
    Some(match output {
        Some(output) => Row::Rendered(output),
        None => Row::Original(text.len()),
    })
}

fn expand_tabs(text: &str) -> Option<String> {
    let mut length = 0;
    tab_widths(text, |c, spaces| length += if spaces > 0 { spaces } else { c.len_utf8() });
    if length > crate::spool::MEMORY {
        return None;
    }
    let mut output = String::with_capacity(length);
    tab_widths(text, |c, spaces| {
        if spaces > 0 {
            output.extend(std::iter::repeat_n(' ', spaces));
        } else {
            output.push(c);
        }
    });
    Some(output)
}

fn tab_widths(text: &str, mut consume: impl FnMut(char, usize)) {
    let mut chars = text.chars().peekable();
    let mut column = 0;
    while let Some(c) = chars.next() {
        if c == '\t' {
            let width = 8 - column % 8;
            column += width;
            // Combining text makes the tab cell no longer a bare tab, so
            // Display::expand_tabs leaves that cell's original text intact.
            let expand = chars.peek().is_none_or(|next| next.width() != Some(0));
            consume(c, if expand { width } else { 0 });
        } else {
            let width = c.width().expect("printable row");
            if width > 0 || column == 0 {
                column += width.max(1);
            }
            consume(c, 0);
        }
    }
}
