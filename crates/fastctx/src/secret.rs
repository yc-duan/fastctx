//! A password typed at the terminal, not shown and not kept in the shell's
//! history (`world-network.md`, 登录与入网). Without a terminal it is the
//! first line of standard input, for scripts that pipe it in; what follows
//! that line stays there for the programs started next, such as sudo
//! reading its own password.

use std::io::{IsTerminal as _, Read as _, Write as _};

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, read};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};

pub fn read_password(prompt: &str) -> anyhow::Result<String> {
    if !std::io::stdin().is_terminal() {
        let line = String::from_utf8(first_line()?)?;
        return Ok(line.trim_end_matches('\r').to_string());
    }
    eprint!("{prompt}");
    std::io::stderr().flush()?;
    enable_raw_mode()?;
    let typed = typed();
    disable_raw_mode()?;
    eprintln!();
    typed
}

fn typed() -> anyhow::Result<String> {
    let mut password = String::new();
    loop {
        let Event::Key(KeyEvent { code, modifiers, kind, .. }) = read()? else { continue };
        if kind == KeyEventKind::Release {
            continue;
        }
        match code {
            KeyCode::Enter => return Ok(password),
            KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => anyhow::bail!("cancelled"),
            KeyCode::Backspace => {
                password.pop();
            }
            KeyCode::Char(character) => password.push(character),
            _ => {}
        }
    }
}

/// The first line of standard input, read a byte at a time straight from it:
/// a buffered read would take what follows the line as well.
fn first_line() -> std::io::Result<Vec<u8>> {
    #[cfg(unix)]
    let mut input = std::fs::File::from(std::os::fd::AsFd::as_fd(&std::io::stdin()).try_clone_to_owned()?);
    #[cfg(windows)]
    let mut input = std::fs::File::from(std::os::windows::io::AsHandle::as_handle(&std::io::stdin()).try_clone_to_owned()?);
    let mut line = Vec::new();
    let mut byte = [0u8];
    while input.read(&mut byte)? == 1 && byte[0] != b'\n' {
        line.push(byte[0]);
    }
    Ok(line)
}

/// A new password, typed twice to be sure of it.
pub fn new_password(prompt: &str) -> anyhow::Result<String> {
    let first = read_password(prompt)?;
    if first.is_empty() {
        anyhow::bail!("the password is empty");
    }
    if !std::io::stdin().is_terminal() {
        return Ok(first);
    }
    if read_password("Again: ")? != first {
        anyhow::bail!("the two passwords differ");
    }
    Ok(first)
}
