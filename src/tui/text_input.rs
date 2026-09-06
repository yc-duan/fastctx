//! Grapheme-aware text editing with bounded, atomic paste and a horizontally scrolling view.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::text::Span;
use unicode_segmentation::UnicodeSegmentation;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum InputMode {
    SingleLine,
    Multiline,
    Token,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum InputError {
    TooLong { limit: usize, count: usize },
    SingleLine,
}

/// Cursor and selection are UTF-8 byte boundaries; movement/deletion follows grapheme clusters.
#[derive(Clone)]
pub(crate) struct TextInput {
    text: String,
    cursor: usize,
    anchor: Option<usize>,
    limit: usize,
    mode: InputMode,
    pub(crate) secret: bool,
    pub(crate) revealed: bool,
    pub(crate) error: Option<InputError>,
}

impl std::fmt::Debug for TextInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TextInput")
            .field("characters", &self.len())
            .field("secret", &self.secret)
            .finish_non_exhaustive()
    }
}

impl Drop for TextInput {
    fn drop(&mut self) {
        if self.secret {
            use zeroize::Zeroize;
            self.text.zeroize();
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct InputCell {
    pub(crate) text: String,
    pub(crate) selected: bool,
    pub(crate) cursor: bool,
}

impl TextInput {
    pub(crate) fn new(text: String, limit: usize, mode: InputMode, secret: bool) -> Self {
        Self {
            cursor: text.len(),
            text,
            anchor: None,
            limit,
            mode,
            secret,
            revealed: false,
            error: None,
        }
    }
    pub(crate) fn text(&self) -> &str {
        &self.text
    }
    pub(crate) fn len(&self) -> usize {
        self.text.chars().count()
    }
    pub(crate) fn limit(&self) -> usize {
        self.limit
    }
    pub(crate) fn replace(&mut self, text: String) {
        self.text = text;
        self.cursor = self.text.len();
        self.anchor = None;
        self.error = None;
    }
    fn selection(&self) -> std::ops::Range<usize> {
        let anchor = self.anchor.unwrap_or(self.cursor);
        anchor.min(self.cursor)..anchor.max(self.cursor)
    }
    pub(crate) fn insert(&mut self, value: &str) -> bool {
        let normalized;
        let value = if self.mode == InputMode::Token {
            normalized = value
                .chars()
                .filter(|ch| !ch.is_ascii_whitespace())
                .collect::<String>();
            normalized.as_str()
        } else {
            value
        };
        if self.mode != InputMode::Multiline
            && value
                .chars()
                .any(|ch| ch.is_control() || matches!(ch, '\u{2028}' | '\u{2029}'))
        {
            self.error = Some(InputError::SingleLine);
            return false;
        }
        let selection = self.selection();
        let count =
            self.len() - self.text[selection.clone()].chars().count() + value.chars().count();
        if count > self.limit {
            self.error = Some(InputError::TooLong {
                limit: self.limit,
                count,
            });
            return false;
        }
        self.text.replace_range(selection.clone(), value);
        self.cursor = selection.start + value.len();
        self.cursor = self
            .text
            .grapheme_indices(true)
            .map(|(offset, _)| offset)
            .find(|offset| *offset >= self.cursor)
            .unwrap_or(self.text.len());
        self.anchor = None;
        self.error = None;
        true
    }
    fn move_to(&mut self, target: usize, select: bool) {
        if select {
            if self.anchor.is_none() {
                self.anchor = Some(self.cursor);
            }
        } else {
            self.anchor = None;
        }
        self.cursor = target;
    }
    fn previous(&self) -> usize {
        self.text
            .grapheme_indices(true)
            .map(|(offset, _)| offset)
            .take_while(|offset| *offset < self.cursor)
            .last()
            .unwrap_or(0)
    }
    fn next(&self) -> usize {
        self.text
            .grapheme_indices(true)
            .map(|(offset, _)| offset)
            .find(|offset| *offset > self.cursor)
            .unwrap_or(self.text.len())
    }
    fn word_left(&self) -> usize {
        let mut at = self.cursor;
        let mut word = false;
        for (offset, grapheme) in self.text[..self.cursor].grapheme_indices(true).rev() {
            let space = grapheme.chars().all(char::is_whitespace);
            if word && space {
                break;
            }
            at = offset;
            word |= !space;
        }
        at
    }
    fn word_right(&self) -> usize {
        let mut word = false;
        for (offset, grapheme) in self.text[self.cursor..].grapheme_indices(true) {
            let space = grapheme.chars().all(char::is_whitespace);
            if word && space {
                return self.cursor + offset;
            }
            word |= !space;
        }
        self.text.len()
    }
    fn erase(&mut self, backwards: bool, word: bool) {
        if self.selection().is_empty() {
            self.anchor = Some(self.cursor);
            self.cursor = if backwards {
                if word {
                    self.word_left()
                } else {
                    self.previous()
                }
            } else if word {
                self.word_right()
            } else {
                self.next()
            };
        }
        self.insert("");
    }
    /// Returns true only for an editing key, leaving form navigation to the caller.
    pub(crate) fn handle_key(&mut self, key: KeyEvent) -> bool {
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        match key.code {
            KeyCode::Char('a') if control => {
                self.anchor = Some(0);
                self.cursor = self.text.len();
            }
            KeyCode::Char('u') if control => {
                self.replace(String::new());
            }
            KeyCode::Char('w') if control => self.erase(true, true),
            KeyCode::Char('j') if control && self.mode == InputMode::Multiline => {
                self.insert("\n");
            }
            KeyCode::Left => {
                let range = self.selection();
                let target = if !shift && !range.is_empty() {
                    range.start
                } else if control {
                    self.word_left()
                } else {
                    self.previous()
                };
                self.move_to(target, shift);
            }
            KeyCode::Right => {
                let range = self.selection();
                let target = if !shift && !range.is_empty() {
                    range.end
                } else if control {
                    self.word_right()
                } else {
                    self.next()
                };
                self.move_to(target, shift);
            }
            KeyCode::Home => self.move_to(0, shift),
            KeyCode::End => self.move_to(self.text.len(), shift),
            KeyCode::Backspace => self.erase(true, control),
            KeyCode::Delete => self.erase(false, control),
            KeyCode::Char(ch)
                if !control && !key.modifiers.contains(KeyModifiers::ALT) && !ch.is_control() =>
            {
                self.insert(&ch.to_string());
            }
            _ => return false,
        }
        true
    }

    /// Builds visible graphemes with a reserved cursor cell. No secret or control byte is
    /// handed to the terminal renderer; selection remains visible in monochrome terminals.
    pub(crate) fn cells(&self, width: usize) -> Vec<InputCell> {
        if width == 0 {
            return Vec::new();
        }
        let selection = self.selection();
        let mut cells = Vec::new();
        let mut cursor_index = 0;
        for (offset, grapheme) in self.text.grapheme_indices(true) {
            if offset < self.cursor {
                cursor_index = cells.len() + 1;
            }
            let text = if self.secret && !self.revealed {
                "●".into()
            } else {
                visible(grapheme)
            };
            cells.push(InputCell {
                text,
                selected: selection.contains(&offset),
                cursor: offset == self.cursor,
            });
        }
        if self.cursor == self.text.len() {
            cells.push(InputCell {
                text: " ".into(),
                selected: false,
                cursor: true,
            });
        } else if let Some(cell) = cells.get_mut(cursor_index) {
            cell.cursor = true;
        }
        let measure = |cell: &InputCell| Span::raw(cell.text.as_str()).width().max(1);
        let widths = cells.iter().map(measure).collect::<Vec<_>>();
        let mut cursor_span = widths[..=cursor_index.min(widths.len().saturating_sub(1))]
            .iter()
            .sum::<usize>();
        let mut start = 0;
        while start < cursor_index && cursor_span > width.saturating_sub(1).max(1) {
            cursor_span = cursor_span.saturating_sub(widths[start]);
            start += 1;
        }
        let mut result = Vec::new();
        let mut used = 0;
        if start > 0 && width > 2 {
            result.push(InputCell {
                text: "<".into(),
                selected: false,
                cursor: false,
            });
            used += 1;
        }
        for cell in cells.into_iter().skip(start) {
            let size = measure(&cell);
            if used + size > width {
                break;
            }
            used += size;
            result.push(cell);
        }
        result
    }
}

fn visible(grapheme: &str) -> String {
    grapheme.chars().map(|ch| match ch {
        '\n' => "\\n".to_string(), '\r' => "\\r".to_string(), '\t' => "\\t".to_string(),
        ch if ch.is_control() || matches!(ch, '\u{061c}' | '\u{200b}' | '\u{200e}' | '\u{200f}' | '\u{2028}'..='\u{202e}' | '\u{2060}'..='\u{2069}' | '\u{feff}') => format!("\\u{:04x}", ch as u32),
        ch => ch.to_string(),
    }).collect()
}
