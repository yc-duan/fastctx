//! What the user does at the keyboard, read on a thread of its own and
//! handed to the screen as it comes.

#[cfg(windows)]
mod decode;
#[cfg(windows)]
mod windows;

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
    /// The terminal's input could not be read any more.
    Failed(String),
}

/// Starts reading the terminal's input into `sender`.
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

/// Puts the terminal's input back as it was.
pub fn restore() {
    #[cfg(windows)]
    windows::restore();
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
