//! Keys and pastes from the characters a terminal sends.
//!
//! On Windows the console hands over a paste as characters between
//! `ESC [ 200 ~` and `ESC [ 201 ~` only when its virtual-terminal input is
//! on, and crossterm's Windows reader drops characters that come without a
//! key code, so the World reads the console itself and decodes here.
//!
//! Nothing here is cut short or waited out. A paste is whatever lies
//! between its brackets, however long. An escape sequence ends where its
//! grammar says it ends (ECMA-48: parameters, intermediates, one final
//! byte). Whether an `ESC` is the Esc key or the start of a sequence is
//! decided at the end of what the console had ready: a terminal writes a
//! sequence whole, so an `ESC` with nothing after it and nothing more
//! waiting was the key.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::Input;

const PASTE_START: &str = "\x1b[200~";
const PASTE_END: &str = "\x1b[201~";

#[derive(Default)]
pub struct Decoder {
    state: State,
}

#[derive(Default)]
enum State {
    #[default]
    Ground,
    /// An escape sequence so far, from its `ESC`.
    Escape(String),
    /// A paste so far; `closing` holds what may be the start of its end.
    Paste { text: String, closing: String, told: usize },
}

impl Decoder {
    /// Whether characters are being gathered, as a paste or a sequence.
    pub fn gathering(&self) -> bool {
        !matches!(self.state, State::Ground)
    }

    /// One character the terminal sent.
    pub fn char(&mut self, c: char, out: &mut Vec<Input>) {
        match &mut self.state {
            State::Paste { text, closing, .. } => {
                closing.push(c);
                while !PASTE_END.starts_with(closing.as_str()) {
                    let first = closing.remove(0);
                    text.push(first);
                }
                if closing == PASTE_END {
                    let State::Paste { text, .. } = std::mem::take(&mut self.state) else { unreachable!() };
                    out.push(Input::Paste(normalise_newlines(&text)));
                }
            }
            State::Escape(sequence) => {
                if sequence == "\x1b" {
                    match c {
                        '[' | 'O' => sequence.push(c),
                        '\x1b' => out.push(key(KeyCode::Esc, KeyModifiers::NONE)),
                        c => {
                            self.state = State::Ground;
                            out.push(alt(c));
                        }
                    }
                    return;
                }
                sequence.push(c);
                if sequence.starts_with("\x1bO") {
                    let sequence = std::mem::take(sequence);
                    self.state = State::Ground;
                    out.extend(ss3(c).unwrap_or_else(|| loose(&sequence)));
                    return;
                }
                // CSI: parameters 0x30–0x3F, intermediates 0x20–0x2F, then
                // one final byte 0x40–0x7E.
                match c {
                    '\x30'..='\x3f' | '\x20'..='\x2f' => {}
                    '\x40'..='\x7e' => {
                        let sequence = std::mem::take(sequence);
                        self.state = State::Ground;
                        if sequence == PASTE_START {
                            self.state = State::Paste { text: String::new(), closing: String::new(), told: 0 };
                        } else if let Some(input) = csi(&sequence) {
                            out.push(input);
                        }
                    }
                    _ => {
                        // Not a sequence after all: what came is passed on
                        // as typed.
                        let sequence = std::mem::take(sequence);
                        self.state = State::Ground;
                        out.extend(loose(&sequence));
                    }
                }
            }
            State::Ground => match c {
                '\x1b' => self.state = State::Escape("\x1b".to_string()),
                c => out.push(plain(c)),
            },
        }
    }

    /// A key the console reported with its code rather than as characters.
    pub fn key(&mut self, event: KeyEvent, out: &mut Vec<Input>) {
        if event.code == KeyCode::Esc {
            return self.char('\x1b', out);
        }
        if self.gathering() {
            let c = match event.code {
                KeyCode::Char(c) => Some(c),
                KeyCode::Enter => Some('\r'),
                KeyCode::Tab => Some('\t'),
                KeyCode::Backspace => Some('\x7f'),
                _ => None,
            };
            if let Some(c) = c {
                return self.char(c, out);
            }
        }
        out.push(Input::Key(event));
    }

    /// The console has handed over all it had ready; `waiting` says whether
    /// more has come in since.
    pub fn boundary(&mut self, waiting: bool, out: &mut Vec<Input>) {
        match &mut self.state {
            State::Escape(sequence) if !waiting => {
                let sequence = std::mem::take(sequence);
                self.state = State::Ground;
                out.extend(loose(&sequence));
            }
            State::Paste { text, closing, told } => {
                let so_far = text.chars().count() + closing.chars().count();
                if so_far != *told {
                    *told = so_far;
                    out.push(Input::Pasting(so_far));
                }
            }
            _ => {}
        }
    }
}

fn key(code: KeyCode, modifiers: KeyModifiers) -> Input {
    Input::Key(KeyEvent::new(code, modifiers))
}

fn alt(c: char) -> Input {
    match plain(c) {
        Input::Key(event) => Input::Key(KeyEvent::new(event.code, event.modifiers | KeyModifiers::ALT)),
        other => other,
    }
}

/// A character typed on its own.
fn plain(c: char) -> Input {
    match c {
        '\r' | '\n' => key(KeyCode::Enter, KeyModifiers::NONE),
        '\t' => key(KeyCode::Tab, KeyModifiers::NONE),
        '\x08' | '\x7f' => key(KeyCode::Backspace, KeyModifiers::NONE),
        '\0' => key(KeyCode::Char(' '), KeyModifiers::CONTROL),
        '\x01'..='\x1a' => key(KeyCode::Char((c as u8 - 1 + b'a') as char), KeyModifiers::CONTROL),
        c => key(KeyCode::Char(c), KeyModifiers::NONE),
    }
}

/// Characters that began like a sequence and were not one: an `ESC` alone
/// is the Esc key, the rest as typed.
fn loose(sequence: &str) -> Vec<Input> {
    let mut chars = sequence.chars();
    let mut out = Vec::new();
    if chars.next() == Some('\x1b') {
        out.push(key(KeyCode::Esc, KeyModifiers::NONE));
    }
    out.extend(chars.map(plain));
    out
}

/// `ESC O x`: function and cursor keys in application mode.
fn ss3(c: char) -> Option<Vec<Input>> {
    let code = match c {
        'P' => KeyCode::F(1),
        'Q' => KeyCode::F(2),
        'R' => KeyCode::F(3),
        'S' => KeyCode::F(4),
        'A' => KeyCode::Up,
        'B' => KeyCode::Down,
        'C' => KeyCode::Right,
        'D' => KeyCode::Left,
        'H' => KeyCode::Home,
        'F' => KeyCode::End,
        _ => return None,
    };
    Some(vec![key(code, KeyModifiers::NONE)])
}

/// `ESC [ … x`: cursor, editing and function keys, with modifiers.
fn csi(sequence: &str) -> Option<Input> {
    let last = sequence.chars().last()?;
    let body = &sequence[2..sequence.len() - last.len_utf8()];
    let numbers: Vec<u16> = body.split(';').map(|part| part.parse().unwrap_or(0)).collect();
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
    let code = match last {
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
            number @ 11..=15 => KeyCode::F((number - 10) as u8),
            number @ 17..=21 => KeyCode::F((number - 11) as u8),
            number @ 23..=24 => KeyCode::F((number - 12) as u8),
            _ => return None,
        },
        _ => return None,
    };
    Some(Input::Key(KeyEvent::new(code, modifiers)))
}

/// A paste with its line ends as `\n`, whatever the terminal sent.
fn normalise_newlines(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}
