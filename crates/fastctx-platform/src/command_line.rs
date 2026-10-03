//! Windows command lines.
//!
//! A Windows process receives one string, which the program splits into
//! arguments itself. Programs built with the Microsoft C runtime, and MSYS
//! programs such as Git Bash, split by the same rules; quoting each argument
//! for those rules makes the program see exactly the arguments given.

use std::ffi::OsStr;
use std::io;
use std::os::windows::ffi::OsStrExt as _;

/// The command line for `program` with `args`, as UTF-16 without the NUL.
pub fn windows_command_line(program: &OsStr, args: &[&OsStr]) -> io::Result<Vec<u16>> {
    let mut line = Vec::new();
    // The program name is split by different rules: no escapes, and a quote
    // can only surround it whole.
    let program: Vec<u16> = program.encode_wide().collect();
    if program.contains(&u16::from(b'"')) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "a program path cannot contain a double quote"));
    }
    line.push(u16::from(b'"'));
    line.extend_from_slice(&program);
    line.push(u16::from(b'"'));
    for arg in args {
        let arg: Vec<u16> = arg.encode_wide().collect();
        if arg.contains(&0) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "an argument cannot contain NUL"));
        }
        line.push(u16::from(b' '));
        quote(&arg, &mut line);
    }
    Ok(line)
}

/// Appends `arg` quoted for the C runtime's splitting rules.
fn quote(arg: &[u16], line: &mut Vec<u16>) {
    let needs_quotes = arg.is_empty() || arg.iter().any(|&c| c == u16::from(b' ') || c == u16::from(b'\t') || c == u16::from(b'"'));
    if !needs_quotes {
        line.extend_from_slice(arg);
        return;
    }
    line.push(u16::from(b'"'));
    let mut backslashes = 0;
    for &c in arg {
        if c == u16::from(b'\\') {
            backslashes += 1;
            continue;
        }
        if c == u16::from(b'"') {
            // Backslashes before a quote are escapes: double them, then
            // escape the quote itself.
            line.extend(std::iter::repeat_n(u16::from(b'\\'), backslashes * 2 + 1));
        } else {
            line.extend(std::iter::repeat_n(u16::from(b'\\'), backslashes));
        }
        backslashes = 0;
        line.push(c);
    }
    // Backslashes before the closing quote would escape it: double them.
    line.extend(std::iter::repeat_n(u16::from(b'\\'), backslashes * 2));
    line.push(u16::from(b'"'));
}
