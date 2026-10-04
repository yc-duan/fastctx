//! Converts native console records without interpreting native keys as VT text.

use std::io;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use windows_sys::Win32::System::Console::{
    KEY_EVENT_RECORD, LEFT_ALT_PRESSED, LEFT_CTRL_PRESSED, RIGHT_ALT_PRESSED, RIGHT_CTRL_PRESSED, SHIFT_PRESSED,
};

use super::{Input, decode::Decoder};

#[derive(Default)]
pub(super) struct Keys {
    surrogate: Option<(u16, u16, u16, KeyModifiers)>,
}

impl Keys {
    pub fn pending(&self) -> bool {
        self.surrogate.is_some()
    }

    pub fn record(&mut self, event: &KEY_EVENT_RECORD, decoder: &mut Decoder, out: &mut Vec<Input>) -> io::Result<()> {
        // SAFETY: KEY_EVENT_RECORD uses the Unicode member in ReadConsoleInputW.
        let unit = unsafe { event.uChar.UnicodeChar };
        let vk = event.wVirtualKeyCode;
        // Alt+numpad commits its character on Alt release.
        let alt_code = vk == 0x12 && event.bKeyDown == 0 && unit != 0;
        if event.bKeyDown == 0 && !alt_code {
            return Ok(());
        }
        let mut modifiers = modifiers(event.dwControlKeyState);
        if alt_code {
            modifiers.remove(KeyModifiers::ALT);
        }
        if (0x60..=0x69).contains(&vk) && modifiers == KeyModifiers::ALT {
            return Ok(());
        }
        // Modifier transitions can occur between UTF-16 units. They carry no text.
        if matches!(vk, 0x10..=0x14 | 0x5b..=0x5c | 0x90..=0x91) && unit == 0 {
            return Ok(());
        }
        let repeats = event.wRepeatCount;
        if repeats == 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "console key has zero repeat count"));
        }
        let c = match unit {
            0xd800..=0xdbff => {
                if self.surrogate.replace((unit, repeats, vk, modifiers)).is_some() {
                    return Err(invalid_utf16());
                }
                return Ok(());
            }
            0xdc00..=0xdfff => {
                let (high, count, source, flags) = self.surrogate.take().ok_or_else(invalid_utf16)?;
                if count != repeats || source != vk || flags != modifiers {
                    return Err(invalid_utf16());
                }
                char::from_u32(0x10000 + ((u32::from(high) - 0xd800) << 10) + u32::from(unit) - 0xdc00)
            }
            _ => {
                if self.surrogate.take().is_some() {
                    return Err(invalid_utf16());
                }
                char::from_u32(u32::from(unit))
            }
        };
        // ConPTY can synthesize an unmappable Unicode character as an Alt-code
        // release, including each half of a surrogate pair. It is text, also
        // inside a paste, rather than a separate physical Alt key.
        if vk == 0 || vk == 0xe7 || alt_code {
            if let Some(c) = c {
                for _ in 0..repeats {
                    decoder.char(c, out);
                }
            }
            return Ok(());
        }
        // Windows reports AltGr as right Alt + left Ctrl. Its printable text
        // belongs to the keyboard layout rather than to a Ctrl+Alt shortcut.
        if event.dwControlKeyState & (RIGHT_ALT_PRESSED | LEFT_CTRL_PRESSED) == RIGHT_ALT_PRESSED | LEFT_CTRL_PRESSED
            && c.is_some_and(|c| !c.is_control())
        {
            modifiers.remove(KeyModifiers::CONTROL | KeyModifiers::ALT);
        }
        let code = native_code(vk, c, modifiers);
        if let Some(code) = code {
            for _ in 0..repeats {
                decoder.key(KeyEvent::new(code, modifiers), out);
            }
        }
        Ok(())
    }
}

fn invalid_utf16() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "console supplied an invalid or mismatched UTF-16 surrogate pair")
}

fn modifiers(state: u32) -> KeyModifiers {
    let mut modifiers = KeyModifiers::NONE;
    if state & SHIFT_PRESSED != 0 {
        modifiers |= KeyModifiers::SHIFT;
    }
    if state & (LEFT_ALT_PRESSED | RIGHT_ALT_PRESSED) != 0 {
        modifiers |= KeyModifiers::ALT;
    }
    if state & (LEFT_CTRL_PRESSED | RIGHT_CTRL_PRESSED) != 0 {
        modifiers |= KeyModifiers::CONTROL;
    }
    modifiers
}

fn native_code(vk: u16, c: Option<char>, modifiers: KeyModifiers) -> Option<KeyCode> {
    Some(match vk {
        0x08 => KeyCode::Backspace,
        0x09 if modifiers.contains(KeyModifiers::SHIFT) => KeyCode::BackTab,
        0x09 => KeyCode::Tab,
        0x0d => KeyCode::Enter,
        0x1b => KeyCode::Esc,
        0x21 => KeyCode::PageUp,
        0x22 => KeyCode::PageDown,
        0x23 => KeyCode::End,
        0x24 => KeyCode::Home,
        0x25 => KeyCode::Left,
        0x26 => KeyCode::Up,
        0x27 => KeyCode::Right,
        0x28 => KeyCode::Down,
        0x2d => KeyCode::Insert,
        0x2e => KeyCode::Delete,
        code @ 0x70..=0x87 => KeyCode::F((code - 0x6f) as u8),
        _ if modifiers.contains(KeyModifiers::CONTROL) => KeyCode::Char(match c? {
            // The native identity disambiguates control codes such as Ctrl+I
            // from Tab and Ctrl+[ from Esc, when the host preserves it.
            '\0' if vk == 0x20 => ' ',
            '\0'..='\x1f' if (0x41..=0x5a).contains(&vk) => char::from_u32(u32::from(vk) + 0x20)?,
            '\0' if vk == 0x32 => '2',
            c @ '\x1b'..='\x1f' => char::from_u32(c as u32 + 0x40)?,
            '\0' => return None,
            c => c,
        }),
        _ => KeyCode::Char(c.filter(|c| *c != '\0')?),
    })
}
