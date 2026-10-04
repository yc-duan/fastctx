//! What the user does at the keyboard, read on a thread of its own and
//! handed to the screen as it comes.

mod decode;
mod sequence;
#[cfg(unix)]
mod unix;
#[cfg(any(unix, test))]
mod utf8;
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
/// Call [`restore`] before disabling raw mode or replacing stdin.
/// Paste line endings are normalized to LF. Hosts may discard characters,
/// modifiers, or paste boundaries before delivery; those cannot be recovered.
/// Native keys interleaved with a paste follow its complete `Paste` event;
/// characters without a native identity cannot be separated from paste text.
/// Unix reads UTF-8 bytes; both platforms use the same VT decoder.
pub fn start(sender: UnboundedSender<Input>) -> std::io::Result<()> {
    crate::terminal::require_interactive()?;
    #[cfg(windows)]
    {
        windows::start(sender)
    }
    #[cfg(unix)]
    {
        unix::start(sender)
    }
}

/// Stops and joins the reader, restoring console mode bits or descriptor flags
/// owned by this module. Idempotent; does not disable the caller's raw mode.
pub fn restore() -> std::io::Result<()> {
    #[cfg(windows)]
    {
        windows::restore()
    }
    #[cfg(unix)]
    {
        unix::restore()
    }
}

#[cfg(test)]
mod tests;
