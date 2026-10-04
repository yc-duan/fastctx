//! What the user does at the keyboard, read on a thread of its own and
//! handed to the screen as it comes.

mod decode;
mod focus;
mod sequence;
#[cfg(unix)]
mod unix;
#[cfg(any(unix, test))]
mod utf8;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
mod windows_keys;

use std::sync::atomic::{AtomicBool, Ordering};

use crossterm::event::{DisableFocusChange, EnableFocusChange, KeyEvent};
use tokio::sync::mpsc::UnboundedSender;

pub use focus::{Focus, focus};

static REPORTING_FOCUS: AtomicBool = AtomicBool::new(false);

#[derive(Debug)]
pub enum Input {
    Key(KeyEvent),
    /// Text pasted whole, its line ends as `\n`.
    Paste(String),
    /// A paste is arriving; this many characters so far.
    Pasting(usize),
    Resize,
    /// A terminal focus report, never text from inside a bracketed paste.
    Focus(Focus),
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
/// Unix reads UTF-8 bytes; both platforms use the same VT decoder. Focus
/// reporting is enabled after the reader's input modes are ready and disabled
/// by [`restore`]. No terminal capability queries are sent.
pub fn start(sender: UnboundedSender<Input>) -> std::io::Result<()> {
    crate::terminal::require_interactive()?;
    #[cfg(windows)]
    {
        windows::start(sender)?;
    }
    #[cfg(unix)]
    {
        unix::start(sender)?;
    }
    // Own the reset even if writing the enable sequence fails partway through.
    REPORTING_FOCUS.store(true, Ordering::Release);
    if let Err(error) = crossterm::execute!(std::io::stdout(), EnableFocusChange) {
        restore()?;
        return Err(error);
    }
    Ok(())
}

/// Stops and joins the reader, restoring console mode bits or descriptor flags
/// owned by this module. Idempotent; does not disable the caller's raw mode.
pub fn restore() -> std::io::Result<()> {
    let reporting = if REPORTING_FOCUS.swap(false, Ordering::AcqRel) {
        crossterm::execute!(std::io::stdout(), DisableFocusChange)
    } else {
        Ok(())
    };
    #[cfg(windows)]
    let reader = windows::restore();
    #[cfg(unix)]
    let reader = unix::restore();
    focus::set(Focus::Unknown);
    reader.and(reporting)
}

fn send(sender: &UnboundedSender<Input>, event: Input) -> Result<(), tokio::sync::mpsc::error::SendError<Input>> {
    if let Input::Focus(value) = event {
        focus::set(value);
    } else if matches!(event, Input::Failed(_)) {
        focus::set(Focus::Unknown);
    }
    sender.send(event)
}

#[cfg(test)]
mod tests;
