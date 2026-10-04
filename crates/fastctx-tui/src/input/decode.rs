//! Decodes the VT character stream shared by Windows and Unix terminals.
//!
//! Paste framing has neither a size limit nor a timeout. An escape prefix is
//! resolved when the terminal queue is empty after a read, without a timed wait.
//! A terminal must enqueue the prefix and its continuation together for that
//! boundary rule to distinguish a key from a sequence.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::Input;

pub(super) const PASTE_START: &str = "\x1b[200~";
pub(super) const PASTE_END: &str = "\x1b[201~";

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
    Paste { text: String, closing: String, count: usize, told: usize, keys: Vec<KeyEvent> },
}

impl Decoder {
    /// One character the terminal sent.
    pub fn char(&mut self, c: char, out: &mut Vec<Input>) {
        match &mut self.state {
            State::Paste { text, closing, count, .. } => {
                *count += 1;
                closing.push(c);
                while !PASTE_END.starts_with(closing.as_str()) {
                    let first = closing.remove(0);
                    text.push(first);
                }
                if closing == PASTE_END {
                    let State::Paste { text, keys, .. } = std::mem::take(&mut self.state) else { unreachable!() };
                    out.push(Input::Paste(normalise_newlines(text)));
                    out.extend(keys.into_iter().map(Input::Key));
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
                    out.push(super::sequence::ss3(c).unwrap_or(Input::Sequence(sequence)));
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
                            self.state = State::Paste {
                                text: String::new(),
                                closing: String::new(),
                                count: 0,
                                told: 0,
                                keys: Vec::new(),
                            };
                        } else {
                            out.push(super::sequence::csi(&sequence).unwrap_or(Input::Sequence(sequence)));
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

    /// A native key with a known identity is never protocol text. Keys arriving
    /// during a paste follow the completed paste, which is delivered atomically.
    #[cfg(any(windows, test))]
    pub fn key(&mut self, event: KeyEvent, out: &mut Vec<Input>) {
        if let State::Paste { keys, .. } = &mut self.state {
            keys.push(event);
        } else {
            if let State::Escape(sequence) = std::mem::take(&mut self.state) {
                out.extend(loose(&sequence));
            }
            out.push(Input::Key(event));
        }
    }

    /// The terminal has handed over all it had ready; `waiting` says whether
    /// more has come in since.
    pub fn boundary(&mut self, waiting: bool, out: &mut Vec<Input>) {
        match &mut self.state {
            State::Escape(sequence) if !waiting => {
                let sequence = std::mem::take(sequence);
                self.state = State::Ground;
                out.extend(loose(&sequence));
            }
            State::Paste { count, closing, told, .. } => {
                // The possible closing delimiter is not yet known to be text.
                let so_far = *count - closing.len();
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

/// A character typed on its own in raw mode. Some legacy encodings are
/// inherently ambiguous (Tab/Ctrl+I, Enter/Ctrl+M, Esc/Ctrl+[).
fn plain(c: char) -> Input {
    match c {
        '\r' => key(KeyCode::Enter, KeyModifiers::NONE),
        '\t' => key(KeyCode::Tab, KeyModifiers::NONE),
        '\x7f' => key(KeyCode::Backspace, KeyModifiers::NONE),
        '\0' => key(KeyCode::Char(' '), KeyModifiers::CONTROL),
        '\x01'..='\x1a' => key(KeyCode::Char((c as u8 - 1 + b'a') as char), KeyModifiers::CONTROL),
        '\x1c'..='\x1f' => key(KeyCode::Char((c as u8 + 0x40) as char), KeyModifiers::CONTROL),
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

/// Canonical line endings for the text editor, in one pass and one allocation.
pub(super) fn normalise_newlines(text: String) -> String {
    if !text.contains('\r') {
        return text;
    }
    let mut result = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\r' {
            if chars.peek() == Some(&'\n') {
                chars.next();
            }
            result.push('\n');
        } else {
            result.push(c);
        }
    }
    result
}
