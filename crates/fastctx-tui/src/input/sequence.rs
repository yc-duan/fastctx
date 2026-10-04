//! Key sequences with explicit, validated parameters. Unknown sequences are
//! returned to the caller instead of being mistaken for a different key.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::Input;

pub(super) fn ss3(c: char) -> Option<Input> {
    Some(key(cursor(c)?, KeyModifiers::NONE))
}

fn cursor(c: char) -> Option<KeyCode> {
    Some(match c {
        'A' => KeyCode::Up,
        'B' => KeyCode::Down,
        'C' => KeyCode::Right,
        'D' => KeyCode::Left,
        'H' => KeyCode::Home,
        'F' => KeyCode::End,
        'P' => KeyCode::F(1),
        'Q' => KeyCode::F(2),
        'R' => KeyCode::F(3),
        'S' => KeyCode::F(4),
        _ => return None,
    })
}

fn modifiers(value: u32) -> Option<KeyModifiers> {
    let bits = value.checked_sub(1)?;
    if bits > 7 {
        return None;
    }
    let mut result = KeyModifiers::NONE;
    if bits & 1 != 0 {
        result |= KeyModifiers::SHIFT;
    }
    if bits & 2 != 0 {
        result |= KeyModifiers::ALT;
    }
    if bits & 4 != 0 {
        result |= KeyModifiers::CONTROL;
    }
    Some(result)
}

fn key(code: KeyCode, modifiers: KeyModifiers) -> Input {
    Input::Key(KeyEvent::new(code, modifiers))
}

fn character(value: u32) -> Option<KeyCode> {
    Some(match value {
        8 | 127 => KeyCode::Backspace,
        9 => KeyCode::Tab,
        13 => KeyCode::Enter,
        27 => KeyCode::Esc,
        value => KeyCode::Char(char::from_u32(value)?),
    })
}

pub(super) fn csi(sequence: &str) -> Option<Input> {
    let last = sequence.chars().last()?;
    let body = &sequence[2..sequence.len() - last.len_utf8()];
    let numbers: Vec<u32> =
        if body.is_empty() { Vec::new() } else { body.split(';').map(str::parse).collect::<Result<_, _>>().ok()? };
    match (last, numbers.as_slice()) {
        ('I', []) => Some(Input::Focus(super::Focus::Focused)),
        ('O', []) => Some(Input::Focus(super::Focus::Unfocused)),
        ('Z', []) => Some(key(KeyCode::BackTab, KeyModifiers::SHIFT)),
        ('u', [value]) => Some(key(character(*value)?, KeyModifiers::NONE)),
        ('u', [value, flags]) | ('~', [27, flags, value]) => Some(key(character(*value)?, modifiers(*flags)?)),
        ('~', [number]) => Some(key(tilde(*number)?, KeyModifiers::NONE)),
        ('~', [number, flags]) => Some(key(tilde(*number)?, modifiers(*flags)?)),
        (_, []) | (_, [1]) => Some(key(cursor(last)?, KeyModifiers::NONE)),
        (_, [1, flags]) => Some(key(cursor(last)?, modifiers(*flags)?)),
        _ => None,
    }
}

fn tilde(number: u32) -> Option<KeyCode> {
    Some(match number {
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
    })
}
