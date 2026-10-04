//! The Windows console read directly. VT input preserves paste framing, but
//! cannot recover modifiers or characters already discarded by the host.

use std::io;
use std::sync::Mutex;
use std::thread::JoinHandle;

use tokio::sync::mpsc::UnboundedSender;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE, WAIT_FAILED, WAIT_OBJECT_0};
use windows_sys::Win32::System::Console::{
    ENABLE_MOUSE_INPUT, ENABLE_VIRTUAL_TERMINAL_INPUT, ENABLE_WINDOW_INPUT, GetConsoleMode,
    GetNumberOfConsoleInputEvents, GetStdHandle, INPUT_RECORD, KEY_EVENT, ReadConsoleInputW, STD_INPUT_HANDLE,
    SetConsoleMode, WINDOW_BUFFER_SIZE_EVENT,
};
use windows_sys::Win32::System::Threading::{CreateEventW, INFINITE, SetEvent, WaitForMultipleObjects};

use super::{Input, decode::Decoder, windows_keys::Keys};

// VT input can turn inherited mouse reporting into character sequences even
// though this reader handles only keys, pastes, and resize notifications.
const ENABLED: u32 = ENABLE_VIRTUAL_TERMINAL_INPUT | ENABLE_WINDOW_INPUT;
const BITS: u32 = ENABLED | ENABLE_MOUSE_INPUT;
static READER: Mutex<Option<Reader>> = Mutex::new(None);

struct Reader {
    input: usize,
    before: u32,
    stop: usize,
    thread: JoinHandle<()>,
}

pub(super) fn start(sender: UnboundedSender<Input>) -> io::Result<()> {
    let mut reader = READER.lock().map_err(|_| io::Error::other("console reader lock poisoned"))?;
    if reader.is_some() {
        return Err(io::Error::new(io::ErrorKind::AlreadyExists, "console input is already being read"));
    }
    // SAFETY: GetStdHandle borrows the process input handle; the caller must
    // not close or replace it while the reader is running.
    let input = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
    if input.is_null() || input == INVALID_HANDLE_VALUE {
        return Err(io::Error::other("this program has no console input"));
    }
    let mut mode = 0;
    // SAFETY: the handle is borrowed and mode is writable.
    if unsafe { GetConsoleMode(input, &mut mode) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: an unnamed, manually reset event with default security.
    let stop = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
    if stop.is_null() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: input was validated by GetConsoleMode.
    if unsafe { SetConsoleMode(input, (mode & !BITS) | ENABLED) } == 0 {
        let error = io::Error::last_os_error();
        unsafe { CloseHandle(stop) };
        return Err(error);
    }
    let input = input as usize;
    let stop = stop as usize;
    super::focus::set(super::Focus::Unknown);
    let thread = std::thread::Builder::new().name("console input".into()).spawn(move || {
        if let Err(error) = read(input as HANDLE, stop as HANDLE, &sender) {
            let _ = super::send(&sender, Input::Failed(error.to_string()));
        }
    });
    match thread {
        Ok(thread) => {
            *reader = Some(Reader { input, before: mode & BITS, stop, thread });
            Ok(())
        }
        Err(error) => {
            let restored = restore_mode(input as HANDLE, mode & BITS);
            // SAFETY: no thread was started and this handle is owned here.
            unsafe { CloseHandle(stop as HANDLE) };
            restored?;
            Err(error)
        }
    }
}

pub(super) fn restore() -> io::Result<()> {
    let mut slot = READER.lock().map_err(|_| io::Error::other("console reader lock poisoned"))?;
    let Some(reader) = slot.as_ref() else { return Ok(()) };
    // SAFETY: the owned stop handle stays open until the thread is joined.
    if unsafe { SetEvent(reader.stop as HANDLE) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let reader = slot.take().unwrap();
    let joined = reader.thread.join();
    let restored = restore_mode(reader.input as HANDLE, reader.before);
    // SAFETY: the reader has exited and no longer uses the owned stop handle.
    let closed = unsafe { CloseHandle(reader.stop as HANDLE) };
    if closed == 0 {
        return Err(io::Error::last_os_error());
    }
    restored?;
    joined.map_err(|_| io::Error::other("console input thread panicked"))
}

fn restore_mode(input: HANDLE, before: u32) -> io::Result<()> {
    let mut mode = 0;
    // SAFETY: input is borrowed from the session; mode is writable.
    if unsafe { GetConsoleMode(input, &mut mode) } == 0
        || unsafe { SetConsoleMode(input, (mode & !BITS) | before) } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn read(input: HANDLE, stop: HANDLE, sender: &UnboundedSender<Input>) -> io::Result<()> {
    let mut decoder = Decoder::default();
    let mut keys = Keys::default();
    // This is an I/O buffer, not a paste limit. Queue emptiness, checked after
    // each read, decides the escape boundary regardless of this buffer size.
    let mut records: [INPUT_RECORD; 256] = unsafe { std::mem::zeroed() };
    let handles = [stop, input];
    loop {
        // SAFETY: both handles remain live until this thread exits.
        match unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, INFINITE) } {
            WAIT_OBJECT_0 => return Ok(()),
            value if value == WAIT_OBJECT_0 + 1 => {}
            WAIT_FAILED => return Err(io::Error::last_os_error()),
            value => return Err(io::Error::other(format!("unexpected console wait result: {value}"))),
        }
        let mut count = 0;
        // SAFETY: records and count are writable; this is the sole input reader.
        if unsafe { ReadConsoleInputW(input, records.as_mut_ptr(), records.len() as u32, &mut count) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut out = Vec::new();
        for record in &records[..count as usize] {
            let result = if record.EventType == KEY_EVENT as u16 {
                // SAFETY: EventType selects KeyEvent.
                keys.record(unsafe { &record.Event.KeyEvent }, &mut decoder, &mut out)
            } else {
                if record.EventType == WINDOW_BUFFER_SIZE_EVENT as u16 {
                    out.push(Input::Resize);
                }
                Ok(())
            };
            // Deliver valid earlier events even if a subsequent record is bad.
            if let Err(error) = result {
                for event in out {
                    let _ = super::send(sender, event);
                }
                return Err(error);
            }
        }
        let mut waiting = 0;
        // SAFETY: waiting is writable and input is the live console handle.
        let checked = unsafe { GetNumberOfConsoleInputEvents(input, &mut waiting) };
        let error = (checked == 0).then(io::Error::last_os_error);
        if checked != 0 {
            decoder.boundary(waiting > 0 || keys.pending(), &mut out);
        }
        for event in out {
            if super::send(sender, event).is_err() {
                return Ok(());
            }
        }
        if let Some(error) = error {
            return Err(error);
        }
    }
}
