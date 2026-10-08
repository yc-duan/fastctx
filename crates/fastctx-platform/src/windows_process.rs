//! What every Windows process creation here shares: the command line and
//! the environment block.
//!
//! A Windows process receives one command-line string, which the program
//! splits into arguments itself. Quoting each argument for the Microsoft C
//! runtime's rules makes such a program see exactly the arguments given.
//! MSYS programs such as Git Bash split by rules of their own: outside
//! double quotes they strip single quotes and expand braces and wildcards,
//! and inside them they also halve doubled backslashes. So an argument with
//! any of those characters is put in double quotes, which the C runtime
//! reads the same either way; one that holds backslashes as well as
//! characters needing quotes does not reach an MSYS program unchanged.

use std::cmp::Ordering;
use std::ffi::{OsStr, OsString};
use std::io;
use std::os::windows::ffi::OsStrExt as _;
use std::path::Path;

use windows_sys::Win32::Globalization::{CSTR_EQUAL, CSTR_GREATER_THAN, CompareStringOrdinal};

/// `path` as a NUL-terminated UTF-16 string.
pub fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain([0]).collect()
}

/// The NUL-terminated command line for `program` with `args`.
pub fn command_line(program: &OsStr, args: &[&OsStr]) -> io::Result<Vec<u16>> {
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
    line.push(0);
    Ok(line)
}

/// Appends `arg` quoted for the C runtime's splitting rules, and in quotes
/// whenever MSYS would read it otherwise.
fn quote(arg: &[u16], line: &mut Vec<u16>) {
    const SPECIAL: &[u8] = b" \t\"'{}*?[]";
    let needs_quotes = arg.is_empty() || arg.iter().any(|&c| SPECIAL.iter().any(|&s| c == u16::from(s)));
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

/// `NAME=value` strings, each NUL-terminated, sorted by name without regard
/// to case as Windows requires, ending with one more NUL. The names were
/// checked by the caller (`crate::detach::check_environment`).
pub fn environment_block(env: &[(OsString, OsString)]) -> Vec<u16> {
    let mut vars: Vec<(Vec<u16>, Vec<u16>)> =
        env.iter().map(|(name, value)| (name.encode_wide().collect(), value.encode_wide().collect())).collect();
    vars.sort_by(|(a, _), (b, _)| compare_names(a, b));
    let mut block = Vec::new();
    for (name, value) in vars {
        block.extend(name);
        block.push(u16::from(b'='));
        block.extend(value);
        block.push(0);
    }
    if block.is_empty() {
        block.push(0);
    }
    block.push(0);
    block
}

fn compare_names(a: &[u16], b: &[u16]) -> Ordering {
    // SAFETY: both slices are valid for their lengths, which fit in i32 for
    // any name the system accepts.
    match unsafe { CompareStringOrdinal(a.as_ptr(), a.len() as i32, b.as_ptr(), b.len() as i32, 1) } {
        CSTR_EQUAL => Ordering::Equal,
        CSTR_GREATER_THAN => Ordering::Greater,
        _ => Ordering::Less,
    }
}
