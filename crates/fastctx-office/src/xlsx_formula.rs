//! A1 reference translation for OOXML shared formulas. Quoted text,
//! workbook names and structured references are copied verbatim.
use anyhow::{Result, ensure};
use office_oxide::xlsx::CellRef;

pub(crate) fn translate(formula: &str, from: &CellRef, to: &CellRef) -> Result<String> {
    let row_delta = i64::from(to.row) - i64::from(from.row);
    let col_delta = i64::from(to.col) - i64::from(from.col);
    let mut output = String::new();
    let mut at = 0;
    let bytes = formula.as_bytes();
    while at < bytes.len() {
        let start = at;
        if matches!(bytes[at], b'\'' | b'"') {
            let quote = bytes[at];
            at += 1;
            while at < bytes.len() {
                if bytes[at] == quote {
                    at += 1;
                    if bytes.get(at) == Some(&quote) {
                        at += 1;
                    } else {
                        break;
                    }
                } else {
                    at += 1;
                }
            }
            output.push_str(&formula[start..at]);
            continue;
        }
        if bytes[at] == b'[' {
            let mut depth = 1;
            at += 1;
            while at < bytes.len() && depth > 0 {
                match bytes[at] {
                    b'[' => depth += 1,
                    b']' => depth -= 1,
                    _ => {}
                }
                at += 1;
            }
            output.push_str(&formula[start..at]);
            continue;
        }
        let before = formula[..at].chars().next_back();
        if before.is_none_or(|c| !identifier(c)) {
            if let Some(end) = sheet_prefix(formula, at) {
                output.push_str(&formula[at..end]);
                at = end;
                continue;
            }
            if let Some((end, address)) = address(formula, at)
                && boundary(formula, end)
                && !matches!(bytes.get(end), Some(b'!' | b'(' | b'['))
            {
                output.push_str(&address.shift(row_delta, col_delta)?);
                at = end;
                continue;
            }
            if let Some((end, text)) = axis_range(formula, at, row_delta, col_delta)? {
                output.push_str(&text);
                at = end;
                continue;
            }
        }
        let c = formula[at..].chars().next().unwrap();
        output.push(c);
        at += c.len_utf8();
    }
    Ok(output)
}

fn identifier(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '.' | '$' | '\\')
}
fn boundary(text: &str, at: usize) -> bool {
    text[at..].chars().next().is_none_or(|c| !identifier(c))
}
fn sheet_prefix(text: &str, at: usize) -> Option<usize> {
    for (offset, c) in text[at..].char_indices() {
        if c == '!' {
            return (offset > 0).then_some(at + offset + 1);
        }
        if !c.is_alphanumeric() && !matches!(c, '_' | '.' | ':' | '$') {
            return None;
        }
    }
    None
}

struct Address {
    row: u32,
    col: u32,
    fixed_row: bool,
    fixed_col: bool,
    lower: bool,
}
impl Address {
    fn shift(&self, row_delta: i64, col_delta: i64) -> Result<String> {
        let row = shifted(self.row, self.fixed_row, row_delta, 1_048_576)?;
        let col = shifted(self.col, self.fixed_col, col_delta, 16_384)?;
        let col = CellRef::col_name(col);
        Ok(format!(
            "{}{}{}{}",
            if self.fixed_col { "$" } else { "" },
            if self.lower { col.to_ascii_lowercase() } else { col },
            if self.fixed_row { "$" } else { "" },
            row + 1
        ))
    }
}
fn shifted(value: u32, fixed: bool, delta: i64, count: i64) -> Result<u32> {
    let value = i64::from(value) + if fixed { 0 } else { delta };
    ensure!((0..count).contains(&value), "shared reference moves outside the worksheet grid");
    Ok(value as u32)
}
fn address(text: &str, mut at: usize) -> Option<(usize, Address)> {
    let bytes = text.as_bytes();
    let fixed_col = bytes.get(at) == Some(&b'$');
    at += usize::from(fixed_col);
    let start = at;
    while bytes.get(at).is_some_and(u8::is_ascii_alphabetic) {
        at += 1;
    }
    if at == start {
        return None;
    }
    let letters = &text[start..at];
    let col = CellRef::parse_col(letters)?;
    if col >= 16_384 {
        return None;
    }
    let fixed_row = bytes.get(at) == Some(&b'$');
    at += usize::from(fixed_row);
    let start = at;
    while bytes.get(at).is_some_and(u8::is_ascii_digit) {
        at += 1;
    }
    let row: u32 = text[start..at].parse().ok()?;
    if row == 0 || row > 1_048_576 {
        return None;
    }
    Some((
        at,
        Address { row: row - 1, col, fixed_row, fixed_col, lower: letters.bytes().all(|b| b.is_ascii_lowercase()) },
    ))
}
fn axis(text: &str, mut at: usize, columns: bool) -> Option<(usize, u32, bool, bool)> {
    let bytes = text.as_bytes();
    let fixed = bytes.get(at) == Some(&b'$');
    at += usize::from(fixed);
    let start = at;
    while bytes.get(at).is_some_and(|b| if columns { b.is_ascii_alphabetic() } else { b.is_ascii_digit() }) {
        at += 1;
    }
    if at == start {
        return None;
    }
    let token = &text[start..at];
    let value = if columns { CellRef::parse_col(token)? } else { token.parse::<u32>().ok()?.checked_sub(1)? };
    if value >= if columns { 16_384 } else { 1_048_576 } {
        return None;
    }
    Some((at, value, fixed, columns && token.bytes().all(|b| b.is_ascii_lowercase())))
}
fn axis_range(text: &str, at: usize, row_delta: i64, col_delta: i64) -> Result<Option<(usize, String)>> {
    for columns in [true, false] {
        let Some((left_end, first, fixed_first, lower_first)) = axis(text, at, columns) else { continue };
        let mut middle = left_end;
        while text.as_bytes().get(middle).is_some_and(u8::is_ascii_whitespace) {
            middle += 1;
        }
        if text.as_bytes().get(middle) != Some(&b':') {
            continue;
        }
        let mut right = middle + 1;
        while text.as_bytes().get(right).is_some_and(u8::is_ascii_whitespace) {
            right += 1;
        }
        let Some((end, last, fixed_last, lower_last)) = axis(text, right, columns) else { continue };
        if !boundary(text, end) || text.as_bytes().get(end) == Some(&b'!') {
            continue;
        }
        let delta = if columns { col_delta } else { row_delta };
        let count = if columns { 16_384 } else { 1_048_576 };
        let format = |value, fixed, lower| -> Result<String> {
            let value = shifted(value, fixed, delta, count)?;
            let text = if columns { CellRef::col_name(value) } else { (value + 1).to_string() };
            Ok(format!("{}{}", if fixed { "$" } else { "" }, if lower { text.to_ascii_lowercase() } else { text }))
        };
        return Ok(Some((
            end,
            format!(
                "{}{}{}",
                format(first, fixed_first, lower_first)?,
                &text[left_end..right],
                format(last, fixed_last, lower_last)?
            ),
        )));
    }
    Ok(None)
}
