//! Terminal admission and the capabilities advertised to this process.
//!
//! A terminal name describes the nearest terminal, which may be a
//! multiplexer. Environment inherited from its parent cannot upgrade it.

#[cfg(windows)]
mod windows;

use std::io::{self, IsTerminal};

use crate::design::Depth;

/// Checks both streams before raw mode, alternate screen, or an input
/// reader is started. `NO_COLOR` does not prohibit an interactive TUI.
///
/// Rust's terminal check also recognizes MSYS/Cygwin PTY pipes on Windows;
/// a `TERM` variable alone never makes a redirected stream interactive.
pub fn require_interactive() -> io::Result<()> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(io::Error::new(io::ErrorKind::Unsupported, "the TUI needs terminal input and output"));
    }
    if variable("TERM") == "dumb" {
        return Err(io::Error::new(io::ErrorKind::Unsupported, "TERM=dumb cannot run a full-screen TUI"));
    }
    Ok(())
}

pub(crate) fn depth() -> io::Result<Depth> {
    let term = variable("TERM");
    if !io::stdout().is_terminal() || present("NO_COLOR") || term == "dumb" {
        return Ok(Depth::Mono);
    }

    let multiplexed = term.starts_with("screen") || term.starts_with("tmux") || present("TMUX") || present("STY");
    if !multiplexed {
        // A Linux console and an explicitly selected ANSI/VT terminal
        // must not inherit the capabilities of the terminal that launched it.
        if let Some(depth) = fixed_depth(&term) {
            return Ok(depth);
        }
        #[cfg(windows)]
        if let Some(depth) = windows::console_depth()? {
            if depth == Depth::Ansi16 {
                return Ok(depth);
            }
            // A remote ConPTY exposes the server's Console API even when
            // the client terminal has fewer colours.
            if term.is_empty() && !present("SSH_CONNECTION") && !present("SSH_TTY") {
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
        io::Error::new(io::ErrorKind::Unsupported, format!("no colour capability information for TERM={term:?}"))
    })
}

fn variable(name: &str) -> String {
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
