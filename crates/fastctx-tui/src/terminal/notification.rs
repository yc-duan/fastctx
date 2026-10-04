//! Notification transports identified without asking the input stream for a
//! reply. Multiplexers and unrecognized identities have no transport.

use std::io::{self, Write};

use crate::input::Focus;

/// The single notification protocol selected for this terminal. This describes
/// transport support, not OS permission or a promise that a banner is shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotificationProtocol {
    /// iTerm2 and kitty's simple notification: title and body become one line.
    Osc9,
    /// WezTerm, Ghostty and Konsole's title/body notification.
    Osc777,
    /// VS Code's internal notification, with separate title and body chunks.
    Osc99,
}

pub(super) fn detect() -> Option<NotificationProtocol> {
    #[cfg(windows)]
    if super::windows::has_visible_window() {
        // A native conhost does not gain notifications from inherited GUI
        // terminal variables. ConPTY's console window is not visible.
        return None;
    }
    identify(super::colors::variable)
}

fn identify(get: impl Fn(&str) -> String) -> Option<NotificationProtocol> {
    let term = get("TERM");
    if matches!(term.as_str(), "dumb" | "ansi" | "linux" | "cygwin")
        || term.starts_with("vt")
        || term.starts_with("screen")
        || term.starts_with("tmux")
        || ["TMUX", "STY", "ZELLIJ"].iter().any(|name| !get(name).is_empty())
    {
        return None;
    }

    let program = get("TERM_PROGRAM");
    let version = get("TERM_PROGRAM_VERSION");
    match program.as_str() {
        "vscode" => return (version_number(&version)? >= [1, 110, 0]).then_some(NotificationProtocol::Osc99),
        "wezterm" => {
            let date = version.split('-').next()?;
            return (date.len() == 8 && date.parse::<u32>().ok()? >= 20210314).then_some(NotificationProtocol::Osc777);
        }
        "ghostty" => return Some(NotificationProtocol::Osc777),
        "iterm.app" => return (version_number(&version)? >= [3, 5, 0]).then_some(NotificationProtocol::Osc9),
        "kitty" => return Some(NotificationProtocol::Osc9),
        "" => {}
        _ => return None,
    }

    // WT_SESSION is a GUID, not a version. WT 1.24/1.25 have no notifications;
    // 1.26 introduces OSC 777. An installed package cannot identify this pane.
    if !get("WT_SESSION").is_empty() {
        return None;
    }
    if term == "xterm-kitty" {
        return Some(NotificationProtocol::Osc9);
    }
    if term == "xterm-ghostty" {
        return Some(NotificationProtocol::Osc777);
    }
    let konsole = get("KONSOLE_VERSION").parse::<u32>().ok()?;
    (konsole >= 221200).then_some(NotificationProtocol::Osc777)
}

fn version_number(version: &str) -> Option<[u32; 3]> {
    let mut parts = version.split('-').next()?.split('.');
    let value = [parts.next()?.parse().ok()?, parts.next()?.parse().ok()?, parts.next()?.parse().ok()?];
    parts.next().is_none().then_some(value)
}

pub(super) fn notify(
    output: &mut impl Write,
    protocol: Option<NotificationProtocol>,
    focus: impl Fn() -> Focus,
    title: &str,
    body: &str,
) -> io::Result<bool> {
    let Some(protocol) = protocol else { return Ok(false) };
    if focus() != Focus::Unfocused {
        return Ok(false);
    }
    let message = encode(protocol, title, body);
    // Formatting a long message can take time. Use the reader's latest report
    // at the write, independently of how many UI events are still queued.
    if focus() != Focus::Unfocused {
        return Ok(false);
    }
    output.write_all(message.as_bytes())?;
    output.flush()?;
    Ok(true)
}

fn encode(protocol: NotificationProtocol, title: &str, body: &str) -> String {
    let clean = |text: &str| -> String {
        text.chars()
            .map(|c| match c {
                c if c.is_control() || matches!(c, '\u{2028}' | '\u{2029}') => ' ',
                ';' if protocol != NotificationProtocol::Osc99 => ',',
                c => c,
            })
            .collect()
    };
    let title = clean(title);
    let body = clean(body);
    match protocol {
        NotificationProtocol::Osc9 => format!("\x1b]9;{title}: {body}\x1b\\"),
        NotificationProtocol::Osc777 => format!("\x1b]777;notify;{title};{body}\x1b\\"),
        NotificationProtocol::Osc99 => {
            let mut message = String::new();
            chunks(&mut message, "title", &title, false);
            chunks(&mut message, "body", &body, true);
            message
        }
    }
}

fn chunks(message: &mut String, payload: &str, mut text: &str, done: bool) {
    // OSC 99 specifies at most 2048 unencoded payload bytes per chunk:
    // https://sw.kovidgoyal.net/kitty/desktop-notifications/
    const PAYLOAD_BYTES: usize = 2048;
    loop {
        let mut end = text.len().min(PAYLOAD_BYTES);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        let (chunk, rest) = text.split_at(end);
        let finished = u8::from(done && rest.is_empty());
        // No identifier means distinct notifications on successive calls.
        // Explicitly request focus only, never activation or close reports.
        message.push_str(&format!("\x1b]99;p={payload}:d={finished}:a=focus:c=0:o=unfocused;{chunk}\x1b\\"));
        if rest.is_empty() {
            break;
        }
        text = rest;
    }
}

#[cfg(test)]
#[path = "notification_tests.rs"]
mod tests;
