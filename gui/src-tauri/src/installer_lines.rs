//! Lines for World's NSIS installer and uninstaller, which run this program
//! with `nsExec` and show its output in their details. `nsExec` reads output
//! as the system's ANSI code page unless it is UTF-16, so the lines are
//! written as UTF-16LE (without a byte-order mark, which it would show) and
//! paths in any language read as they are.

/// A line on standard output.
pub(crate) fn say(line: &str) {
    write(&mut std::io::stdout().lock(), line);
}

/// A line on standard error, which `nsExec` reads together with output.
pub(crate) fn complain(line: &str) {
    write(&mut std::io::stderr().lock(), line);
}

fn write(stream: &mut impl std::io::Write, line: &str) {
    let bytes: Vec<u8> = line.encode_utf16().chain("\r\n".encode_utf16()).flat_map(u16::to_le_bytes).collect();
    // The uninstaller's details are the only reader; a closed stream has
    // no one to tell.
    let _ = stream.write_all(&bytes).and_then(|()| stream.flush());
}
