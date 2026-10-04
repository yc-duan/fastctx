//! Where the user types: one line or several, moved through by grapheme,
//! laid out by display width. The terminal's own cursor is put where the
//! caret is, so an input method shows what is being composed right there.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_segmentation::UnicodeSegmentation;

use crate::text;

#[derive(Clone, Debug, Default)]
pub struct Editor {
    text: String,
    /// The caret, as a byte offset on a grapheme boundary.
    caret: usize,
    multiline: bool,
}

/// What a key did to the text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edit {
    Changed,
    Moved,
    /// Enter, to send or to go on.
    Enter,
    /// Not a key for the editor: the page may use it.
    Unhandled,
}

impl Editor {
    pub fn single() -> Self {
        Self::default()
    }

    pub fn multi() -> Self {
        Self { multiline: true, ..Self::default() }
    }

    pub fn with(mut self, text: &str) -> Self {
        self.set(text);
        self
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty()
    }

    pub fn set(&mut self, text: &str) {
        self.text = self.fit(text);
        self.caret = self.text.len();
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.caret = 0;
    }

    /// Text as it may stand here: one line keeps no line ends.
    fn fit(&self, text: &str) -> String {
        if self.multiline { text.to_string() } else { text.replace(['\r', '\n'], " ") }
    }

    pub fn insert(&mut self, text: &str) {
        let text = self.fit(text);
        self.text.insert_str(self.caret, &text);
        self.caret += text.len();
    }

    pub fn key(&mut self, key: &KeyEvent) -> Edit {
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::Enter if self.multiline && (alt || key.modifiers.contains(KeyModifiers::SHIFT)) => {
                self.insert("\n");
                Edit::Changed
            }
            KeyCode::Char('j') if control && self.multiline => {
                self.insert("\n");
                Edit::Changed
            }
            // A backslash right before the caret asks for a new line instead,
            // as in Claude Code: Windows Terminal takes Alt+Enter for itself,
            // and many terminals send Shift+Enter as a plain Enter.
            KeyCode::Enter if self.multiline && self.text[..self.caret].ends_with('\\') => {
                self.text.replace_range(self.caret - 1..self.caret, "\n");
                Edit::Changed
            }
            KeyCode::Enter => Edit::Enter,
            KeyCode::Char('u') if control => {
                let start = self.line_start();
                self.text.replace_range(start..self.caret, "");
                self.caret = start;
                Edit::Changed
            }
            KeyCode::Char('w') if control => {
                let start = self.word_before();
                self.text.replace_range(start..self.caret, "");
                self.caret = start;
                Edit::Changed
            }
            KeyCode::Char('a') if control => {
                self.caret = self.line_start();
                Edit::Moved
            }
            KeyCode::Char('e') if control => {
                self.caret = self.line_end();
                Edit::Moved
            }
            KeyCode::Char(c) if !control && !alt => {
                let mut buffer = [0; 4];
                self.insert(c.encode_utf8(&mut buffer));
                Edit::Changed
            }
            KeyCode::Tab if self.multiline => {
                self.insert("\t");
                Edit::Changed
            }
            KeyCode::Backspace => match self.before() {
                Some(start) => {
                    self.text.replace_range(start..self.caret, "");
                    self.caret = start;
                    Edit::Changed
                }
                None => Edit::Moved,
            },
            KeyCode::Delete => match self.after() {
                Some(end) => {
                    self.text.replace_range(self.caret..end, "");
                    Edit::Changed
                }
                None => Edit::Moved,
            },
            KeyCode::Left if control => {
                self.caret = self.word_before();
                Edit::Moved
            }
            KeyCode::Right if control => {
                self.caret = self.word_after();
                Edit::Moved
            }
            KeyCode::Left => {
                self.caret = self.before().unwrap_or(self.caret);
                Edit::Moved
            }
            KeyCode::Right => {
                self.caret = self.after().unwrap_or(self.caret);
                Edit::Moved
            }
            KeyCode::Home => {
                self.caret = self.line_start();
                Edit::Moved
            }
            KeyCode::End => {
                self.caret = self.line_end();
                Edit::Moved
            }
            KeyCode::Up if self.multiline && self.line_start() > 0 => {
                self.vertical(-1);
                Edit::Moved
            }
            KeyCode::Down if self.multiline && self.line_end() < self.text.len() => {
                self.vertical(1);
                Edit::Moved
            }
            _ => Edit::Unhandled,
        }
    }

    fn before(&self) -> Option<usize> {
        self.text[..self.caret].grapheme_indices(true).next_back().map(|(index, _)| index)
    }

    fn after(&self) -> Option<usize> {
        self.text[self.caret..].graphemes(true).next().map(|grapheme| self.caret + grapheme.len())
    }

    fn line_start(&self) -> usize {
        self.text[..self.caret].rfind('\n').map_or(0, |index| index + 1)
    }

    fn line_end(&self) -> usize {
        self.text[self.caret..].find('\n').map_or(self.text.len(), |index| self.caret + index)
    }

    fn word_before(&self) -> usize {
        let head = &self.text[..self.caret];
        let trimmed = head.trim_end();
        trimmed.unicode_word_indices().next_back().map_or(0, |(index, _)| index)
    }

    fn word_after(&self) -> usize {
        let tail = &self.text[self.caret..];
        let mut words = tail.unicode_word_indices();
        match words.next() {
            Some((index, word)) => self.caret + index + word.len(),
            None => self.text.len(),
        }
    }

    /// Moves the caret a line up or down, keeping its column on screen.
    fn vertical(&mut self, direction: i32) {
        let start = self.line_start();
        let column = text::width(&self.text[start..self.caret]);
        let target_start = if direction < 0 {
            self.text[..start.saturating_sub(1)].rfind('\n').map_or(0, |index| index + 1)
        } else {
            self.line_end() + 1
        };
        let target_end = self.text[target_start..].find('\n').map_or(self.text.len(), |index| target_start + index);
        let mut caret = target_start;
        let mut used = 0;
        for (index, grapheme) in self.text[target_start..target_end].grapheme_indices(true) {
            let cells = text::width_at(grapheme, used);
            if used + cells > column {
                break;
            }
            used += cells;
            caret = target_start + index + grapheme.len();
        }
        self.caret = caret;
    }

    /// The text laid out in `width` cells, and where the caret is in it as
    /// (row, column). A line wider than `width` is cut where it reaches it.
    pub fn layout(&self, width: usize, style: Style) -> (Vec<Line<'static>>, (usize, usize)) {
        let width = width.max(2);
        let mut lines = vec![String::new()];
        let mut used = 0;
        let mut source_column = 0;
        let mut caret = (0, 0);
        let mut placed = false;
        for (index, grapheme) in self.text.grapheme_indices(true) {
            if index == self.caret {
                caret = (lines.len() - 1, used);
                placed = true;
            }
            if grapheme == "\n" || grapheme == "\r\n" {
                lines.push(String::new());
                used = 0;
                source_column = 0;
                continue;
            }
            let cells = text::width_at(grapheme, source_column);
            source_column += cells;
            if grapheme == "\t" {
                // Tab stops belong to the source line, as they do when
                // measuring the caret. Expanded spaces may span visual rows.
                for space in 0..cells {
                    if used >= width {
                        lines.push(String::new());
                        used = 0;
                        if index == self.caret && space == 0 {
                            caret = (lines.len() - 1, 0);
                        }
                    }
                    lines.last_mut().expect("a line").push(' ');
                    used += 1;
                }
                continue;
            }
            if used + cells > width && used > 0 {
                lines.push(String::new());
                used = 0;
                if index == self.caret {
                    caret = (lines.len() - 1, 0);
                }
            }
            // Keep the displayed form (including a base for an isolated mark)
            // so adding the page's input gutter cannot change its width.
            lines.last_mut().expect("a line").push_str(&text::fit(grapheme, cells));
            used += cells;
        }
        if !placed {
            if used >= width {
                lines.push(String::new());
                used = 0;
            }
            caret = (lines.len() - 1, used);
        }
        (lines.into_iter().map(|line| Line::from(Span::styled(line, style))).collect(), caret)
    }
}
