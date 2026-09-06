//! VT keyboard and bracketed-paste decoding for Windows console input.
//!
//! ConPTY preserves paste delimiters only with VT input enabled. Crossterm's Windows adapter
//! discards control characters with virtual-key code zero, so the native reader supplies
//! their Unicode values directly. This keeps paste from acting as navigation. (2026-09-06)

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use std::time::{Duration, Instant};
const END: &str = "\x1b[201~";
const MAX_PASTE_CHARS: usize = 64 * 1024;
pub(super) enum Decoded {
    Event(Event),
    Rejected(Option<usize>),
}
#[derive(Default)]
pub(super) struct PasteDecoder {
    escape: String,
    paste: Option<String>,
    closing: String,
    count: usize,
    last: Option<Instant>,
}

impl PasteDecoder {
    pub(super) fn is_collecting(&self) -> bool {
        self.paste.is_some() || !self.escape.is_empty()
    }
    pub(super) fn character(&mut self, ch: char) -> Vec<Decoded> {
        self.last = Some(Instant::now());
        if self.escape == "\x1b" && ch == '\x1b' {
            return vec![decoded_key(KeyCode::Esc, KeyModifiers::NONE)];
        }
        if self.paste.is_some() {
            self.closing.push(ch);
            while !END.starts_with(&self.closing) {
                let first = self.closing.remove(0);
                self.count += 1;
                if self.count <= MAX_PASTE_CHARS
                    && let Some(paste) = &mut self.paste
                {
                    paste.push(first);
                }
            }
            if self.closing == END {
                self.closing.clear();
                let paste = self.paste.take().unwrap_or_default();
                let result = if self.count > MAX_PASTE_CHARS {
                    Decoded::Rejected(Some(self.count))
                } else {
                    Decoded::Event(Event::Paste(paste))
                };
                self.count = 0;
                return vec![result];
            }
            return Vec::new();
        }
        if !self.escape.is_empty() {
            self.escape.push(ch);
            if self.escape.len() == 2 && (ch == '[' || ch == 'O') {
                return Vec::new();
            }
            if self.escape.starts_with("\x1b[") {
                if self.escape.len() > 64 {
                    self.escape.clear();
                    return Vec::new();
                }
                if !(('@'..='~').contains(&ch)) {
                    return Vec::new();
                }
                let sequence = std::mem::take(&mut self.escape);
                if sequence == "\x1b[200~" {
                    self.paste = Some(String::new());
                    self.count = 0;
                    return Vec::new();
                }
                return csi(&sequence).into_iter().map(Decoded::Event).collect();
            }
            if self.escape.starts_with("\x1bO") {
                self.escape.clear();
                let code = match ch {
                    'P' => Some(KeyCode::F(1)),
                    'Q' => Some(KeyCode::F(2)),
                    'R' => Some(KeyCode::F(3)),
                    'S' => Some(KeyCode::F(4)),
                    'A' => Some(KeyCode::Up),
                    'B' => Some(KeyCode::Down),
                    'C' => Some(KeyCode::Right),
                    'D' => Some(KeyCode::Left),
                    'H' => Some(KeyCode::Home),
                    'F' => Some(KeyCode::End),
                    _ => None,
                };
                return code
                    .map(|code| decoded_key(code, KeyModifiers::NONE))
                    .into_iter()
                    .collect();
            }
            self.escape.clear();
            return vec![decoded_key(KeyCode::Char(ch), KeyModifiers::ALT)];
        }
        if ch == '\x1b' {
            self.escape.push(ch);
            return Vec::new();
        }
        let (code, modifiers) = match ch {
            '\r' | '\n' => (KeyCode::Enter, KeyModifiers::NONE),
            '\t' => (KeyCode::Tab, KeyModifiers::NONE),
            '\x08' | '\x7f' => (KeyCode::Backspace, KeyModifiers::NONE),
            '\0' => (KeyCode::Char(' '), KeyModifiers::CONTROL),
            ch if ('\x01'..='\x1a').contains(&ch) => (
                KeyCode::Char(char::from(ch as u8 - 1 + b'a')),
                KeyModifiers::CONTROL,
            ),
            ch => (KeyCode::Char(ch), KeyModifiers::NONE),
        };
        vec![decoded_key(code, modifiers)]
    }
    pub(super) fn key(&mut self, key: KeyEvent) -> Vec<Decoded> {
        if key.code == KeyCode::Esc {
            return self.character('\x1b');
        }
        if key.code == KeyCode::Char('[') && key.modifiers.contains(KeyModifiers::ALT) {
            let mut result = self.character('\x1b');
            result.extend(self.character('['));
            return result;
        }
        if self.is_collecting() {
            let ch = match key.code {
                KeyCode::Char(ch) => Some(ch),
                KeyCode::Enter => Some('\n'),
                KeyCode::Tab => Some('\t'),
                KeyCode::Backspace => Some('\x08'),
                _ => None,
            };
            if let Some(ch) = ch {
                return self.character(ch);
            }
        }
        vec![Decoded::Event(Event::Key(key))]
    }
    pub(super) fn expired(&mut self) -> Vec<Decoded> {
        let Some(last) = self.last else {
            return Vec::new();
        };
        if self.paste.is_some() && last.elapsed() >= Duration::from_secs(2) {
            *self = Self::default();
            vec![Decoded::Rejected(None)]
        } else if !self.escape.is_empty() && last.elapsed() >= Duration::from_millis(50) {
            let escape = std::mem::take(&mut self.escape);
            self.last = None;
            if escape == "\x1b" {
                vec![decoded_key(KeyCode::Esc, KeyModifiers::NONE)]
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        }
    }
}
fn decoded_key(code: KeyCode, modifiers: KeyModifiers) -> Decoded {
    Decoded::Event(Event::Key(KeyEvent::new(code, modifiers)))
}
fn csi(sequence: &str) -> Option<Event> {
    let final_char = sequence.chars().last()?;
    let body = &sequence[2..sequence.len() - 1];
    let numbers = body
        .split(';')
        .map(|part| part.parse::<u16>().unwrap_or(0))
        .collect::<Vec<_>>();
    let flags = numbers.get(1).copied().unwrap_or(1).saturating_sub(1);
    let mut modifiers = KeyModifiers::NONE;
    if flags & 1 != 0 {
        modifiers |= KeyModifiers::SHIFT;
    }
    if flags & 2 != 0 {
        modifiers |= KeyModifiers::ALT;
    }
    if flags & 4 != 0 {
        modifiers |= KeyModifiers::CONTROL;
    }
    let code = match final_char {
        'A' => KeyCode::Up,
        'B' => KeyCode::Down,
        'C' => KeyCode::Right,
        'D' => KeyCode::Left,
        'H' => KeyCode::Home,
        'F' => KeyCode::End,
        'Z' => KeyCode::BackTab,
        'P' => KeyCode::F(1),
        'Q' => KeyCode::F(2),
        'R' => KeyCode::F(3),
        'S' => KeyCode::F(4),
        '~' => match numbers.first().copied().unwrap_or(0) {
            1 | 7 => KeyCode::Home,
            2 => KeyCode::Insert,
            3 => KeyCode::Delete,
            4 | 8 => KeyCode::End,
            5 => KeyCode::PageUp,
            6 => KeyCode::PageDown,
            11..=15 => KeyCode::F((numbers[0] - 10) as u8),
            17..=21 => KeyCode::F((numbers[0] - 11) as u8),
            23 | 24 => KeyCode::F((numbers[0] - 12) as u8),
            _ => return None,
        },
        'I' => return Some(Event::FocusGained),
        'O' => return Some(Event::FocusLost),
        _ => return None,
    };
    Some(Event::Key(KeyEvent::new(code, modifiers)))
}
