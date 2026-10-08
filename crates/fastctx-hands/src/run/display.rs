//! Horizontal terminal layout. Unknown or incomplete instructions remain
//! visible, so an unsupported instruction cannot consume later output.

use super::terminal::Part;
use std::collections::BTreeMap;
use unicode_width::UnicodeWidthChar;

#[derive(Default)]
pub(super) struct Display {
    cells: BTreeMap<u128, Cell>,
    cursor: u128,
    saved: Option<u128>,
    pub view: u64,
    pub legacy: bool,
}

struct Cell {
    text: String,
    view: u64,
}

impl Display {
    pub fn write(&mut self, text: &str, literal: bool) {
        let mut chars = text.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '\r' if !literal => {
                    self.expand_tabs();
                    self.cursor = 0;
                }
                '\u{8}' if !literal => {
                    self.expand_tabs();
                    self.cursor = self.cursor.saturating_sub(1);
                }
                '\t' if !literal => {
                    if let Some(end) = (self.cursor / 8).checked_add(1).and_then(|n| n.checked_mul(8)) {
                        if self.cells.range(self.cursor..end).next().is_none() {
                            self.put("\t".into(), (end - self.cursor) as usize);
                        } else {
                            self.cursor = end;
                        }
                    } else {
                        self.byte(b'\t');
                    }
                }
                '\u{1b}' if !literal => {
                    let mut sequence = String::from("\u{1b}");
                    if chars.peek() == Some(&'[') {
                        sequence.push(chars.next().expect("peeked"));
                        let mut intermediate = false;
                        let mut complete = false;
                        while let Some(&next) = chars.peek() {
                            match next {
                                '\u{30}'..='\u{3f}' if !intermediate => {}
                                '\u{20}'..='\u{2f}' => intermediate = true,
                                '\u{40}'..='\u{7e}' => complete = true,
                                _ => break,
                            }
                            sequence.push(chars.next().expect("peeked"));
                            if complete {
                                break;
                            }
                        }
                        if complete && self.csi(&sequence[2..]) {
                            continue;
                        }
                    } else if let Some(&next) = chars.peek() {
                        match next {
                            '7' => {
                                chars.next();
                                self.saved = Some(self.cursor);
                                continue;
                            }
                            '8' if self.saved.is_some() => {
                                chars.next();
                                self.expand_tabs();
                                self.cursor = self.saved.expect("saved in this row");
                                continue;
                            }
                            _ => {}
                        }
                    }
                    self.write(&sequence, true);
                }
                c if c.is_control() => {
                    if self.legacy {
                        self.byte(c as u8);
                    } else {
                        for byte in c.encode_utf8(&mut [0; 4]).as_bytes() {
                            self.byte(*byte);
                        }
                    }
                }
                c => self.put(c.to_string(), c.width().expect("controls are handled above")),
            }
        }
    }

    /// Escapes are one terminal cell, even though their readable spelling
    /// takes four characters. A later overwrite replaces the whole byte.
    pub fn byte(&mut self, byte: u8) {
        self.put(format!("\\x{byte:02X}"), 1);
    }

    fn csi(&mut self, sequence: &str) -> bool {
        let (parameters, command) = sequence.split_at(sequence.len() - 1);
        match command {
            "m" if parameters.bytes().all(|b| b.is_ascii_digit() || matches!(b, b';' | b':')) => return true,
            "h" | "l" if matches!(parameters, "?25" | "?12" | "?2004") => return true,
            _ => {}
        }
        let value = if parameters.is_empty() {
            0
        } else if let Ok(n) = parameters.parse::<u64>() {
            n
        } else {
            return false;
        };
        let distance = u128::from(value.max(1));
        self.expand_tabs();
        match command {
            "C" | "a" => {
                let Some(cursor) = self.cursor.checked_add(distance) else { return false };
                self.cursor = cursor;
            }
            "D" => self.cursor = self.cursor.saturating_sub(distance),
            "G" | "`" => self.cursor = distance - 1,
            "K" => match value {
                0 => {
                    self.split_wide(self.cursor);
                    self.cells.split_off(&self.cursor);
                }
                1 => {
                    self.split_wide(self.cursor.saturating_add(1));
                    self.cells = self.cells.split_off(&self.cursor.saturating_add(1));
                }
                2 => self.cells.clear(),
                _ => return false,
            },
            "P" => {
                let end = self.cursor.saturating_add(distance);
                self.split_wide(self.cursor);
                self.split_wide(end);
                let rest = self.cells.split_off(&end);
                self.cells.split_off(&self.cursor);
                for (position, cell) in rest {
                    self.cells.insert(position - distance, cell);
                }
            }
            "X" => {
                let end = self.cursor.saturating_add(distance);
                self.split_wide(self.cursor);
                self.split_wide(end);
                let rest = self.cells.split_off(&end);
                self.cells.split_off(&self.cursor);
                self.cells.extend(rest);
            }
            "s" if value == 0 => self.saved = Some(self.cursor),
            "u" if value == 0 => {
                let Some(saved) = self.saved else { return false };
                self.cursor = saved;
            }
            _ => return false,
        }
        true
    }

    fn split_wide(&mut self, at: u128) {
        if self.cells.get(&at).is_some_and(|cell| cell.text.is_empty()) {
            self.cells.get_mut(&at).expect("continuation cell").text = " ".into();
            if let Some(cell) = at.checked_sub(1).and_then(|before| self.cells.get_mut(&before)) {
                cell.text = " ".into();
            }
        }
    }

    /// A tab can stay a tab in ordinary file output. Once columns are
    /// edited, its blank cells must be individually addressable.
    fn expand_tabs(&mut self) {
        let tabs: Vec<_> = self.cells.iter().filter(|(_, cell)| cell.text == "\t").map(|(&at, _)| at).collect();
        for at in tabs {
            self.cells.get_mut(&at).expect("tab").text = " ".into();
            for (_, cell) in self.cells.range_mut(at + 1..) {
                if !cell.text.is_empty() {
                    break;
                }
                cell.text = " ".into();
            }
        }
    }

    fn put(&mut self, text: String, width: usize) {
        if width == 0
            && self.cursor > 0
            && let Some((_, cell)) = self.cells.range_mut(..self.cursor).rev().find(|(_, cell)| !cell.text.is_empty())
        {
            cell.text.push_str(&text);
            // A cell edited outside its view no longer belongs wholly to
            // that view's budget, including added combining characters.
            if cell.view != self.view {
                cell.view = 0;
            }
            return;
        }
        let width = width.max(1) as u128;
        self.split_wide(self.cursor);
        self.split_wide(self.cursor + width);
        self.cells.insert(self.cursor, Cell { text, view: self.view });
        for position in self.cursor + 1..self.cursor + width {
            self.cells.insert(position, Cell { text: String::new(), view: self.view });
        }
        self.cursor = self.cursor.saturating_add(width);
    }

    pub fn take(&mut self) -> std::io::Result<Vec<(u64, Vec<Part>)>> {
        self.cursor = 0;
        self.saved = None;
        let mut groups = Vec::new();
        let mut parts = Vec::new();
        let mut at = 0;
        let mut text = String::new();
        let mut view = self.cells.first_key_value().map_or(self.view, |(_, cell)| cell.view);
        for (position, cell) in std::mem::take(&mut self.cells) {
            if cell.view != view {
                if !text.is_empty() {
                    parts.push(Part::Text(std::mem::take(&mut text)));
                }
                if !parts.is_empty() {
                    groups.push((view, std::mem::take(&mut parts)));
                }
                view = cell.view;
            }
            if position > at {
                if !text.is_empty() {
                    parts.push(Part::Text(std::mem::take(&mut text)));
                }
                let spaces = usize::try_from(position - at).map_err(|_| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "terminal cursor moved beyond the addressable character count",
                    )
                })?;
                parts.push(Part::Spaces(spaces));
            }
            text.push_str(&cell.text);
            at = position + 1;
        }
        if !text.is_empty() {
            parts.push(Part::Text(text));
        }
        if !parts.is_empty() {
            groups.push((view, parts));
        }
        Ok(groups)
    }
}
