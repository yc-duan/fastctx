//! Exact horizontal layout over a bounded cache of sparse terminal cells.

use super::cells::{Cell, Cells};
use super::terminal::Part;
use crate::spool::{BLOCK, Bytes, Spool};
use std::io::{self, Write};
use unicode_width::UnicodeWidthChar;

#[derive(Default)]
pub(super) struct Display {
    cells: Cells,
    cursor: u128,
    saved: Option<u128>,
    pub view: u64,
    pub legacy: bool,
    has_tabs: bool,
}

impl Display {
    pub fn write(&mut self, text: &Bytes, literal: bool) -> io::Result<()> {
        self.write_with_tags(text, literal, None)
    }

    pub fn write_tagged(
        &mut self,
        text: &Bytes,
        literal: bool,
        tags: &[(std::ops::Range<usize>, u64)],
    ) -> io::Result<()> {
        self.write_with_tags(text, literal, Some(tags))
    }

    fn write_with_tags(
        &mut self,
        text: &Bytes,
        literal: bool,
        tags: Option<&[(std::ops::Range<usize>, u64)]>,
    ) -> io::Result<()> {
        let position = std::cell::Cell::new(0usize);
        let mut chars = text
            .chars()
            .map(|character| {
                if let Ok(character) = character {
                    position.set(position.get() + character.len_utf8());
                }
                character
            })
            .peekable();
        while let Some(c) = chars.next() {
            let c = c?;
            if let Some(tags) = tags {
                self.view = source_tag(tags, position.get() - c.len_utf8());
            }
            if c != '\u{1b}' || literal {
                self.character(c, literal)?;
                continue;
            }
            let mut sequence = Spool::default();
            sequence.write_all(b"\x1b")?;
            if chars.peek().is_some_and(|c| matches!(c, Ok('['))) {
                chars.next();
                sequence.write_all(b"[")?;
                let mut intermediate = false;
                let mut complete = None;
                let mut numeric = Some(0u64);
                let mut digits = 0usize;
                let mut sgr = true;
                let mut short = String::new();
                while let Some(Ok(next)) = chars.peek() {
                    let next = *next;
                    match next {
                        '\u{30}'..='\u{3f}' if !intermediate => {}
                        '\u{20}'..='\u{2f}' => intermediate = true,
                        '\u{40}'..='\u{7e}' => complete = Some(next),
                        _ => break,
                    }
                    chars.next();
                    if complete.is_some() {
                        break;
                    }
                    sequence.write_all(next.encode_utf8(&mut [0; 4]).as_bytes())?;
                    if short.len() < 8 {
                        short.push(next);
                    } else {
                        short.clear();
                        short.push('\0');
                    }
                    sgr &= next.is_ascii_digit() || matches!(next, ';' | ':');
                    numeric = if let Some(digit) = next.to_digit(10) {
                        numeric.and_then(|n| n.checked_mul(10)).and_then(|n| n.checked_add(u64::from(digit)))
                    } else {
                        None
                    };
                    digits += 1;
                }
                if let Some(command) = complete {
                    if command == 'm' && sgr {
                        continue;
                    }
                    if matches!(command, 'h' | 'l') && matches!(short.as_str(), "?25" | "?12" | "?2004") {
                        continue;
                    }
                    if let Some(value) = numeric {
                        self.csi(if digits == 0 { 0 } else { value }, command)?;
                    }
                    continue;
                }
            } else if chars.peek().is_some_and(|c| matches!(c, Ok('7'))) {
                chars.next();
                self.saved = Some(self.cursor);
                continue;
            } else if chars.peek().is_some_and(|c| matches!(c, Ok('8'))) {
                chars.next();
                if let Some(saved) = self.saved {
                    self.expand_tabs()?;
                    self.cursor = saved;
                }
                continue;
            } else {
                while chars.peek().is_some_and(|c| matches!(c, Ok('\u{20}'..='\u{2f}'))) {
                    let c = chars.next().expect("peeked")?;
                    sequence.write_all(c.encode_utf8(&mut [0; 4]).as_bytes())?;
                }
                if chars.peek().is_some_and(|c| matches!(c, Ok('\u{30}'..='\u{7e}'))) {
                    chars.next();
                    continue;
                }
            }
            for c in sequence.finish().chars() {
                self.character(c?, true)?;
            }
        }
        Ok(())
    }
    fn character(&mut self, c: char, literal: bool) -> io::Result<()> {
        match c {
            '\r' if !literal => {
                self.expand_tabs()?;
                self.cursor = 0;
            }
            '\u{8}' if !literal => {
                self.expand_tabs()?;
                self.cursor = self.cursor.saturating_sub(1);
            }
            '\t' if !literal => {
                if let Some(end) = (self.cursor / 8).checked_add(1).and_then(|n| n.checked_mul(8)) {
                    if self.cells.next(self.cursor)?.is_none_or(|(at, _)| at >= end) {
                        self.put("\t", (end - self.cursor) as usize)?;
                        self.has_tabs = true;
                    } else {
                        self.cursor = end;
                    }
                } else {
                    self.byte(b'\t')?;
                }
            }
            c if c.is_control() => {
                if self.legacy {
                    self.byte(c as u8)?;
                } else {
                    for byte in c.encode_utf8(&mut [0; 4]).bytes() {
                        self.byte(byte)?;
                    }
                }
            }
            c => self.put(c.encode_utf8(&mut [0; 4]), c.width().expect("printable"))?,
        }
        Ok(())
    }
    pub fn byte(&mut self, byte: u8) -> io::Result<()> {
        self.put(&format!(r"\x{byte:02X}"), 1)
    }
    fn csi(&mut self, value: u64, command: char) -> io::Result<()> {
        let distance = u128::from(value.max(1));
        self.expand_tabs()?;
        match command {
            'C' | 'a' => {
                if let Some(at) = self.cursor.checked_add(distance) {
                    self.cursor = at;
                }
            }
            'D' => self.cursor = self.cursor.saturating_sub(distance),
            'G' | '\u{60}' => self.cursor = distance - 1,
            'K' => match value {
                0 => {
                    self.split_wide(self.cursor)?;
                    self.erase(self.cursor, u128::MAX)?;
                }
                1 => {
                    let end = self.cursor.saturating_add(1);
                    self.split_wide(end)?;
                    self.erase(0, end)?;
                }
                2 => self.cells = Cells::default(),
                _ => {}
            },
            'P' | 'X' => {
                let end = self.cursor.saturating_add(distance);
                self.split_wide(self.cursor)?;
                self.split_wide(end)?;
                self.erase(self.cursor, end)?;
                if command == 'P' {
                    let mut from = end;
                    while let Some((at, cell)) = self.cells.next(from)? {
                        self.cells.remove(at)?;
                        self.cells.set(at - distance, cell)?;
                        let Some(next) = at.checked_add(1) else {
                            break;
                        };
                        from = next;
                    }
                }
            }
            's' if value == 0 => self.saved = Some(self.cursor),
            'u' if value == 0 => {
                if let Some(saved) = self.saved {
                    self.cursor = saved;
                }
            }
            _ => {}
        }
        Ok(())
    }
    fn erase(&mut self, first: u128, end: u128) -> io::Result<()> {
        let mut from = first;
        while let Some((at, _)) = self.cells.next(from)? {
            if at >= end {
                break;
            }
            self.cells.remove(at)?;
            let Some(next) = at.checked_add(1) else {
                break;
            };
            from = next;
        }
        Ok(())
    }
    fn split_wide(&mut self, at: u128) -> io::Result<()> {
        if let Some(cell) = self.cells.get(at)?.filter(|c| c.empty()) {
            self.cells.set(at, Cell::new(" ", cell.view))?;
            if let Some(before) = at.checked_sub(1)
                && let Some(cell) = self.cells.get(before)?
            {
                self.cells.set(before, Cell::new(" ", cell.view))?;
            }
        }
        Ok(())
    }
    fn expand_tabs(&mut self) -> io::Result<()> {
        if !self.has_tabs {
            return Ok(());
        }
        let mut from = 0;
        while let Some((at, cell)) = self.cells.next(from)? {
            if cell.tab() {
                self.cells.set(at, Cell::new(" ", cell.view))?;
                let mut next = at + 1;
                while let Some((position, cell)) = self.cells.next(next)? {
                    if !cell.empty() {
                        break;
                    }
                    self.cells.set(position, Cell::new(" ", cell.view))?;
                    let Some(after) = position.checked_add(1) else {
                        break;
                    };
                    next = after;
                }
            }
            let Some(next) = at.checked_add(1) else {
                break;
            };
            from = next;
        }
        self.has_tabs = false;
        Ok(())
    }
    fn put(&mut self, text: &str, width: usize) -> io::Result<()> {
        if width == 0 && self.cursor > 0 {
            let mut before = self.cursor - 1;
            while let Some((at, mut cell)) = self.cells.previous(before)? {
                if !cell.empty() {
                    self.cells.append(&mut cell, text)?;
                    if cell.view != self.view {
                        cell.view = 0;
                    }
                    self.cells.set(at, cell)?;
                    return Ok(());
                }
                let Some(previous) = at.checked_sub(1) else {
                    break;
                };
                before = previous;
            }
        }
        let width = width.max(1) as u128;
        self.split_wide(self.cursor)?;
        self.split_wide(self.cursor + width)?;
        self.cells.set(self.cursor, Cell::new(text, self.view))?;
        for at in self.cursor + 1..self.cursor + width {
            self.cells.set(at, Cell::new("", self.view))?;
        }
        self.cursor = self.cursor.saturating_add(width);
        Ok(())
    }
    pub fn take(&mut self, mut consume: impl FnMut(u64, Part) -> io::Result<()>) -> io::Result<()> {
        self.cursor = 0;
        self.saved = None;
        let mut from = 0;
        let mut at = 0;
        let mut text = String::new();
        let mut view = self.cells.next(0)?.map_or(self.view, |(_, cell)| cell.view);
        while let Some((position, cell)) = self.cells.next(from)? {
            if (cell.view != view || position > at) && !text.is_empty() {
                consume(view, Part::Text(std::mem::take(&mut text)))?;
            }
            view = cell.view;
            if position > at {
                let spaces = usize::try_from(position - at).map_err(|_| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        "terminal cursor moved beyond the addressable character count",
                    )
                })?;
                consume(view, Part::Spaces(spaces))?;
            }
            self.cells.text(cell, |chunk| {
                text.push_str(chunk);
                if text.len() >= BLOCK {
                    consume(view, Part::Text(std::mem::take(&mut text)))?;
                }
                Ok(())
            })?;
            at = position + 1;
            let Some(next) = position.checked_add(1) else {
                break;
            };
            from = next;
        }
        if !text.is_empty() {
            consume(view, Part::Text(text))?;
        }
        self.cells = Cells::default();
        self.has_tabs = false;
        Ok(())
    }
}

pub(super) fn source_tag(tags: &[(std::ops::Range<usize>, u64)], byte: usize) -> u64 {
    let at = tags.partition_point(|(range, _)| range.end <= byte);
    tags.get(at).filter(|(range, _)| range.contains(&byte)).map_or(0, |(_, tag)| *tag)
}
