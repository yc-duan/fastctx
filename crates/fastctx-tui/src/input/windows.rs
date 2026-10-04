//! The Windows console read directly: its virtual-terminal input keeps a
//! paste's brackets, and keys it reports with their codes keep their
//! modifiers (Shift+Enter among them).

use std::sync::atomic::{AtomicU32, Ordering};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tokio::sync::mpsc::UnboundedSender;
use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Console::{
    ENABLE_VIRTUAL_TERMINAL_INPUT, ENABLE_WINDOW_INPUT, GetConsoleMode, GetNumberOfConsoleInputEvents, GetStdHandle,
    INPUT_RECORD, KEY_EVENT, KEY_EVENT_RECORD, ReadConsoleInputW, STD_INPUT_HANDLE, SetConsoleMode,
    WINDOW_BUFFER_SIZE_EVENT,
};

use super::Input;
use super::decode::Decoder;

/// The mode bits turned on here, and which of them were on before.
const BITS: u32 = ENABLE_VIRTUAL_TERMINAL_INPUT | ENABLE_WINDOW_INPUT;
static BEFORE: AtomicU32 = AtomicU32::new(u32::MAX);

/// Turns on the console's virtual-terminal input and starts reading it.
pub fn start(sender: UnboundedSender<Input>) -> std::io::Result<()> {
    // SAFETY: GetStdHandle borrows the process's input handle.
    let handle = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        return Err(std::io::Error::other("this program has no console input"));
    }
    let mut mode = 0;
    // SAFETY: the handle is the console's input and `mode` is writable.
    if unsafe { GetConsoleMode(handle, &mut mode) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    BEFORE.store(mode & BITS, Ordering::SeqCst);
    // SAFETY: as above.
    if unsafe { SetConsoleMode(handle, mode | BITS) } == 0 {
        let error = std::io::Error::last_os_error();
        restore();
        return Err(error);
    }
    let handle = handle as usize;
    std::thread::Builder::new().name("console input".into()).spawn(move || read(handle as HANDLE, sender))?;
    Ok(())
}

/// Puts back the mode bits as they were; safe to call more than once.
pub fn restore() {
    let before = BEFORE.swap(u32::MAX, Ordering::SeqCst);
    if before == u32::MAX {
        return;
    }
    // SAFETY: as in `start`.
    let handle = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
    let mut mode = 0;
    // SAFETY: as in `start`.
    if unsafe { GetConsoleMode(handle, &mut mode) } != 0 {
        unsafe { SetConsoleMode(handle, (mode & !BITS) | before) };
    }
}

fn read(handle: HANDLE, sender: UnboundedSender<Input>) {
    let mut decoder = Decoder::default();
    let mut surrogate = None;
    // How many records one read takes at most; a batch's end is decided by
    // asking the console whether more waits, not by this number.
    let mut records: Vec<INPUT_RECORD> = vec![unsafe { std::mem::zeroed() }; 256];
    loop {
        let mut count = 0;
        // SAFETY: the buffer is writable for its length, and `count` too.
        if unsafe { ReadConsoleInputW(handle, records.as_mut_ptr(), records.len() as u32, &mut count) } == 0 {
            let _ = sender.send(Input::Failed(std::io::Error::last_os_error().to_string()));
            return;
        }
        let mut out = Vec::new();
        for record in &records[..count as usize] {
            if record.EventType == KEY_EVENT as u16 {
                // SAFETY: the record says it holds a key event.
                key(unsafe { &record.Event.KeyEvent }, &mut decoder, &mut surrogate, &mut out);
            } else if record.EventType == WINDOW_BUFFER_SIZE_EVENT as u16 {
                out.push(Input::Resize);
            }
        }
        let mut waiting = 0;
        // SAFETY: the handle is the console's input and `waiting` writable.
        unsafe { GetNumberOfConsoleInputEvents(handle, &mut waiting) };
        decoder.boundary(waiting > 0, &mut out);
        for input in out {
            if sender.send(input).is_err() {
                return;
            }
        }
    }
}

fn key(event: &KEY_EVENT_RECORD, decoder: &mut Decoder, surrogate: &mut Option<u16>, out: &mut Vec<Input>) {
    if event.bKeyDown == 0 {
        return;
    }
    // SAFETY: a key event's character is read as UTF-16.
    let unit = unsafe { event.uChar.UnicodeChar };
    let c = match unit {
        0xd800..=0xdbff => {
            *surrogate = Some(unit);
            return;
        }
        0xdc00..=0xdfff => surrogate.take().and_then(|high| char::decode_utf16([high, unit]).next()?.ok()),
        unit => {
            *surrogate = None;
            char::from_u32(u32::from(unit))
        }
    };
    let repeats = event.wRepeatCount.max(1);
    // Characters without a key code are what the terminal sent as text:
    // pastes, escape sequences, input methods.
    if event.wVirtualKeyCode == 0 || (decoder.gathering() && unit != 0) {
        if let Some(c) = c {
            for _ in 0..repeats {
                decoder.char(c, out);
            }
        }
        return;
    }
    let state = event.dwControlKeyState;
    let mut modifiers = KeyModifiers::NONE;
    if state & 0x10 != 0 {
        modifiers |= KeyModifiers::SHIFT;
    }
    if state & 0x03 != 0 {
        modifiers |= KeyModifiers::ALT;
    }
    if state & 0x0c != 0 {
        modifiers |= KeyModifiers::CONTROL;
    }
    // AltGr is Ctrl+Alt to the console; with a character it is typing.
    if state & 0x09 == 0x09 && c.is_some_and(|c| !c.is_control()) {
        modifiers.remove(KeyModifiers::CONTROL | KeyModifiers::ALT);
    }
    let code = match event.wVirtualKeyCode {
        0x08 => Some(KeyCode::Backspace),
        0x09 if modifiers.contains(KeyModifiers::SHIFT) => Some(KeyCode::BackTab),
        0x09 => Some(KeyCode::Tab),
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
        code @ 0x70..=0x87 => Some(KeyCode::F((code - 0x6f) as u8)),
        _ => c.filter(|c| *c != '\0').map(|c| {
            KeyCode::Char(if c.is_ascii_control() && modifiers.contains(KeyModifiers::CONTROL) {
                (c as u8 + b'a' - 1) as char
            } else {
                c
            })
        }),
    };
    if let Some(code) = code {
        for _ in 0..repeats {
            decoder.key(KeyEvent::new(code, modifiers), out);
        }
    }
}
