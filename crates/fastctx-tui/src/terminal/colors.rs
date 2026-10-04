//! How many colours the terminal shows, as far as this process is told.
//!
//! A terminal name describes the nearest terminal, which may be a
//! multiplexer. Environment inherited from its parent cannot upgrade it.

use std::io::{self, IsTerminal};

use crate::design::Depth;

pub(crate) fn depth() -> io::Result<Depth> {
    let term = variable("TERM");
    if !io::stdout().is_terminal() || present("NO_COLOR") || term == "dumb" {
        return Ok(Depth::Mono);
    }

    // A Linux console and an explicitly selected ANSI/VT terminal must
    // not inherit the capabilities of the terminal that launched it.
    if let Some(depth) = fixed_depth(&term) {
        return Ok(depth);
    }
    #[cfg(windows)]
    let console = super::windows::console_depth()?;
    #[cfg(windows)]
    if console == Some(Depth::Ansi16) {
        // A bridge that transports only 16 colours also limits a multiplexer.
        return Ok(Depth::Ansi16);
    }
    let multiplexed = term.starts_with("screen") || term.starts_with("tmux") || present("TMUX") || present("STY");
    if !multiplexed {
        #[cfg(windows)]
        if let Some(depth) = console {
            // A remote ConPTY exposes the server's Console API even when
            // the client terminal has fewer colours.
            if (term.is_empty() || super::windows::has_visible_window())
                && !present("SSH_CONNECTION")
                && !present("SSH_TTY")
            {
                return Ok(depth);
            }
        }
        if matches!(variable("COLORTERM").as_str(), "truecolor" | "24bit")
            || matches!(variable("TERM_PROGRAM").as_str(), "vscode" | "mintty" | "iterm.app" | "wezterm" | "ghostty")
            || (present("WT_SESSION") && !present("SSH_CONNECTION") && !present("SSH_TTY"))
        {
            return Ok(Depth::TrueColor);
        }
    }

    #[cfg(unix)]
    match terminfo::Database::from_env() {
        Ok(info) => return Ok(database_depth(&info)),
        Err(terminfo::Error::NotFound) => {}
        Err(error) => return Err(io::Error::other(format!("cannot read terminfo for {term:?}: {error}"))),
    }

    named_depth(&term).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::Unsupported,
            format!(
                "nothing says how many colours this terminal shows (TERM={term:?}); set COLORTERM=truecolor if \
                 it shows 24-bit colour, or NO_COLOR=1 to draw without colour"
            ),
        )
    })
}

pub(super) fn variable(name: &str) -> String {
    std::env::var(name).unwrap_or_default().to_ascii_lowercase()
}

fn present(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|value| !value.is_empty())
}

fn fixed_depth(term: &str) -> Option<Depth> {
    match term {
        "dumb" | "vt100" | "vt102" | "vt220" | "vt320" | "vt420" | "vt520" => Some(Depth::Mono),
        "ansi" | "linux" | "cygwin" => Some(Depth::Ansi16),
        _ => None,
    }
}

fn named_depth(term: &str) -> Option<Depth> {
    if term.ends_with("-direct") || term.ends_with("-truecolor") || term.ends_with("-24bit") {
        Some(Depth::TrueColor)
    } else if term.ends_with("-256color") {
        Some(Depth::Ansi256)
    } else if matches!(term, "xterm" | "screen" | "tmux" | "rxvt") {
        Some(Depth::Ansi16)
    } else {
        fixed_depth(term)
    }
}

#[cfg(unix)]
fn database_depth(info: &terminfo::Database) -> Depth {
    use terminfo::capability::{MaxColors, Value};

    let flag = |name| matches!(info.raw(name), Some(Value::True) | Some(Value::Number(1..)));
    let string = |name| matches!(info.raw(name), Some(Value::String(value)) if !value.is_empty());
    if flag("RGB") || flag("Tc") || (string("setrgbf") && string("setrgbb")) {
        return Depth::TrueColor;
    }
    match info.get::<MaxColors>().map(|MaxColors(colors)| colors) {
        Some(16_777_216..) => Depth::TrueColor,
        Some(256..) => Depth::Ansi256,
        Some(8..) => Depth::Ansi16,
        _ => Depth::Mono,
    }
}
