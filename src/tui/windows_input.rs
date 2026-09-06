//! Windows input preserving VT paste bytes and accepting native console key records.

use super::paste::{Decoded, PasteDecoder};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;
use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE, WAIT_FAILED, WAIT_TIMEOUT};
use windows_sys::Win32::System::Console::{
    ENABLE_VIRTUAL_TERMINAL_INPUT, ENABLE_WINDOW_INPUT, FlushConsoleInputBuffer, GetConsoleMode,
    GetStdHandle, INPUT_RECORD, KEY_EVENT, ReadConsoleInputW, STD_INPUT_HANDLE, SetConsoleMode,
    WINDOW_BUFFER_SIZE_EVENT,
};
use windows_sys::Win32::System::Threading::WaitForSingleObject;
const MODE_BITS: u32 = ENABLE_VIRTUAL_TERMINAL_INPUT | ENABLE_WINDOW_INPUT;
static ORIGINAL_BITS: AtomicU32 = AtomicU32::new(u32::MAX);

/// Restores only the bits owned here, also safe after the pre-unwind raw-mode restoration.
pub(super) fn restore_mode() {
    let saved = ORIGINAL_BITS.swap(u32::MAX, Ordering::SeqCst);
    if saved == u32::MAX {
        return;
    }
    let handle = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
    let mut current = 0;
    // SAFETY: the console handle is borrowed and the mode output is writable.
    if unsafe { GetConsoleMode(handle, &mut current) } != 0 {
        unsafe { SetConsoleMode(handle, (current & !MODE_BITS) | saved) };
    }
}
pub(super) struct WindowsInput {
    handle: HANDLE,
    decoder: PasteDecoder,
    surrogate: Option<u16>,
}
impl WindowsInput {
    pub(super) fn new() -> Result<Self, String> {
        let handle = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            return Err("Cannot open terminal input.".into());
        }
        let mut current = 0;
        if unsafe { GetConsoleMode(handle, &mut current) } == 0 {
            return Err(format!(
                "Cannot inspect terminal input mode: {}",
                std::io::Error::last_os_error()
            ));
        }
        ORIGINAL_BITS.store(current & MODE_BITS, Ordering::SeqCst);
        if unsafe { SetConsoleMode(handle, current | MODE_BITS) } == 0 {
            restore_mode();
            return Err(format!(
                "Cannot enable terminal paste input: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(Self {
            handle,
            decoder: PasteDecoder::default(),
            surrogate: None,
        })
    }
    pub(super) fn read(&mut self, timeout: Duration) -> Result<Vec<Decoded>, String> {
        let mut result = self.decoder.expired();
        if !result.is_empty() {
            return Ok(result);
        }
        // SAFETY: the borrowed console handle remains valid throughout the TUI loop.
        let wait = unsafe {
            WaitForSingleObject(
                self.handle,
                timeout.as_millis().min(u32::MAX as u128) as u32,
            )
        };
        if wait == WAIT_TIMEOUT {
            return Ok(self.decoder.expired());
        }
        if wait == WAIT_FAILED {
            return Err(format!(
                "Cannot wait for terminal input: {}",
                std::io::Error::last_os_error()
            ));
        }
        let mut records: [INPUT_RECORD; 256] = unsafe { std::mem::zeroed() };
        let mut read = 0;
        // SAFETY: the writable record array and count slot have their declared sizes.
        if unsafe {
            ReadConsoleInputW(
                self.handle,
                records.as_mut_ptr(),
                records.len() as u32,
                &mut read,
            )
        } == 0
        {
            return Err(format!(
                "Cannot read terminal input: {}",
                std::io::Error::last_os_error()
            ));
        }
        for record in &records[..read as usize] {
            if record.EventType == KEY_EVENT as u16 {
                let key = unsafe { record.Event.KeyEvent };
                if key.bKeyDown == 0 {
                    continue;
                }
                let unit = unsafe { key.uChar.UnicodeChar };
                let ch = match unit {
                    0xd800..=0xdbff => {
                        self.surrogate = Some(unit);
                        continue;
                    }
                    0xdc00..=0xdfff => self.surrogate.take().and_then(|high| {
                        char::decode_utf16([high, unit]).next().and_then(Result::ok)
                    }),
                    unit => {
                        self.surrogate = None;
                        char::from_u32(u32::from(unit))
                    }
                };
                let repeats = key.wRepeatCount.max(1);
                if key.wVirtualKeyCode == 0 || (self.decoder.is_collecting() && unit != 0) {
                    if let Some(ch) = ch {
                        for _ in 0..repeats {
                            result.extend(self.decoder.character(ch));
                        }
                    }
                    continue;
                }
                let mut modifiers = KeyModifiers::NONE;
                if key.dwControlKeyState & 0x10 != 0 {
                    modifiers |= KeyModifiers::SHIFT;
                }
                if key.dwControlKeyState & 0x03 != 0 {
                    modifiers |= KeyModifiers::ALT;
                }
                if key.dwControlKeyState & 0x0c != 0 {
                    modifiers |= KeyModifiers::CONTROL;
                }
                if key.dwControlKeyState & 0x09 == 0x09 && ch.is_some_and(|ch| !ch.is_control()) {
                    modifiers.remove(KeyModifiers::CONTROL | KeyModifiers::ALT);
                }
                let code = match key.wVirtualKeyCode {
                    0x08 => Some(KeyCode::Backspace),
                    0x09 => Some(if modifiers.contains(KeyModifiers::SHIFT) {
                        KeyCode::BackTab
                    } else {
                        KeyCode::Tab
                    }),
                    0x0d => Some(KeyCode::Enter),
                    0x1b => Some(KeyCode::Esc),
                    0x21 => Some(KeyCode::PageUp),
                    0x22 => Some(KeyCode::PageDown),
                    0x23 => Some(KeyCode::End),
                    0x24 => Some(KeyCode::Home),
                    0x25 => Some(KeyCode::Left),
                    0x26 => Some(KeyCode::Up),
                    0x27 => Some(KeyCode::Right),
                    0x28 => Some(KeyCode::Down),
                    0x2d => Some(KeyCode::Insert),
                    0x2e => Some(KeyCode::Delete),
                    0x70..=0x87 => Some(KeyCode::F((key.wVirtualKeyCode - 0x6f) as u8)),
                    _ => ch.filter(|ch| *ch != '\0').map(|ch| {
                        KeyCode::Char(
                            if ch.is_ascii_control() && modifiers.contains(KeyModifiers::CONTROL) {
                                char::from((ch as u8) + b'a' - 1)
                            } else {
                                ch
                            },
                        )
                    }),
                };
                if let Some(code) = code {
                    for _ in 0..repeats {
                        result.extend(self.decoder.key(KeyEvent::new(code, modifiers)));
                    }
                }
            } else if record.EventType == WINDOW_BUFFER_SIZE_EVENT as u16 {
                let size = unsafe { record.Event.WindowBufferSizeEvent.dwSize };
                result.push(Decoded::Event(Event::Resize(
                    size.X.max(0) as u16,
                    size.Y.max(0) as u16,
                )));
            }
        }
        Ok(result)
    }
    pub(super) fn drain(&mut self) -> Result<(), String> {
        self.decoder = PasteDecoder::default();
        self.surrogate = None;
        if unsafe { FlushConsoleInputBuffer(self.handle) } == 0 {
            Err(format!(
                "Cannot drain terminal input: {}",
                std::io::Error::last_os_error()
            ))
        } else {
            Ok(())
        }
    }
}
impl Drop for WindowsInput {
    fn drop(&mut self) {
        restore_mode();
    }
}
