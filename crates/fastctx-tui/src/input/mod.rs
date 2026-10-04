//! What the user does at the keyboard, read on a thread of its own and
//! handed to the screen as it comes.

#[cfg(any(windows, test))]
mod decode;
#[cfg(any(windows, test))]
mod sequence;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
mod windows_keys;

use crossterm::event::KeyEvent;
use tokio::sync::mpsc::UnboundedSender;

#[derive(Debug)]
pub enum Input {
    Key(KeyEvent),
    /// Text pasted whole, its line ends as `\n`.
    Paste(String),
    /// A paste is arriving; this many characters so far.
    Pasting(usize),
    Resize,
    /// An unrecognized terminal control sequence, preserved verbatim.
    Sequence(String),
    /// The terminal's input could not be read any more.
    Failed(String),
}

/// Starts the sole terminal reader. The caller must first enable raw mode and
/// bracketed paste, and must not run another terminal reader concurrently.
///
/// On Windows, call [`restore`] before disabling raw mode or replacing stdin.
/// Paste line endings are normalized to LF. Hosts may discard characters,
/// modifiers, or paste boundaries before delivery; those cannot be recovered.
/// Native keys interleaved with a paste follow its complete `Paste` event;
/// characters without a native identity cannot be separated from paste text.
/// Linux uses crossterm's parser and inherits its terminal protocol semantics.
pub fn start(sender: UnboundedSender<Input>) -> std::io::Result<()> {
    crate::terminal::require_interactive()?;
    #[cfg(windows)]
    {
        windows::start(sender)
    }
    #[cfg(not(windows))]
    {
        std::thread::Builder::new().name("terminal input".into()).spawn(move || read(sender))?;
        Ok(())
    }
}

/// On Windows, stops and joins the reader and restores the console mode bits
/// owned by this module. Idempotent; does not disable the caller's raw mode.
/// On Unix, the crossterm reader lives until process exit and this is a no-op.
pub fn restore() -> std::io::Result<()> {
    #[cfg(windows)]
    {
        windows::restore()
    }
    #[cfg(not(windows))]
    {
        Ok(())
    }
}

#[cfg(not(windows))]
fn read(sender: UnboundedSender<Input>) {
    use crossterm::event::{Event, KeyEventKind};
    loop {
        let input = match crossterm::event::read() {
            Ok(Event::Key(key)) if key.kind != KeyEventKind::Release => Input::Key(key),
            Ok(Event::Paste(text)) => Input::Paste(text.replace("\r\n", "\n").replace('\r', "\n")),
            Ok(Event::Resize(..)) => Input::Resize,
            Ok(_) => continue,
            Err(error) => Input::Failed(error.to_string()),
        };
        let failed = matches!(input, Input::Failed(_));
        if sender.send(input).is_err() || failed {
            return;
        }
    }
}

#[cfg(test)]
mod tests;
