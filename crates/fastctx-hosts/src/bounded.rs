//! The largest record line of a host World reads (design §28.49). A line
//! longer than `LINE_MOST` bytes, or holding more than `VALUES_MOST` JSON
//! values, is not parsed: reading it is an error, so whatever reads the
//! record stops before it and fails as it fails on any read it cannot make
//! — nothing is judged gone, no seam moves past it, and no comparison is
//! made against a record half read. A line within the bounds is read whole,
//! as ever.

use std::io::{self, BufRead};

/// The most bytes of one line read.
pub const LINE_MOST: usize = 32 << 20;
/// The most structure one line may hold to be parsed, counted before it is
/// as its container openings and the commas between elements and members
/// outside strings (`too_many_values`): a budget of how much a parse of it
/// builds, about its JSON values — each value of a container but its first
/// counts, and each container — not their exact number.
pub const VALUES_MOST: usize = 100_000;

/// Why a line is not read.
pub fn refused(line: usize, why: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("line {line} of the host's record {why}, more than World reads; World reads this conversation up to it"),
    )
}

/// Why a record whose line is not counted is not read (`refused`).
pub fn refused_record(why: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("a record of the host {why}, more than World reads"))
}

/// Whether `error` is a line refused for its size (`refused`).
pub fn is_refused(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::InvalidData && error.to_string().contains("more than World reads")
}

/// What `read_bounded` read.
pub enum Read {
    /// A line within the bound, its bytes: up to and with its newline, or a
    /// last line without one; none at the end.
    Line(usize),
    /// A line longer than `LINE_MOST`: what of it was read is consumed, its
    /// newline among it where `ended`.
    Refused { ended: bool },
}

/// Reads into `line` up to and with its newline, or finds the line longer
/// than `LINE_MOST` (`Read::Refused`).
pub fn read_bounded(reader: &mut impl BufRead, line: &mut Vec<u8>) -> io::Result<Read> {
    let mut read = 0;
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return Ok(Read::Line(read));
        }
        let (take, done) = match available.iter().position(|&byte| byte == b'\n') {
            Some(at) => (at + 1, true),
            None => (available.len(), false),
        };
        if line.len() + take > LINE_MOST + 1 {
            reader.consume(take);
            return Ok(Read::Refused { ended: done });
        }
        line.extend_from_slice(&available[..take]);
        reader.consume(take);
        read += take;
        if done {
            return Ok(Read::Line(read));
        }
    }
}

/// Reads into `line` up to and with its newline: the bytes read, or, where
/// the line is longer than `LINE_MOST`, an error — what of it was read is
/// consumed, as a line read whole would be.
pub fn read_line(reader: &mut impl BufRead, line: &mut Vec<u8>, number: usize) -> io::Result<usize> {
    match read_bounded(reader, line)? {
        Read::Line(read) => Ok(read),
        Read::Refused { .. } => Err(refused(number, &format!("is longer than {LINE_MOST} bytes"))),
    }
}

/// Takes what is left of a line refused for its length (`Read::Refused`
/// that did not end), to its newline, without keeping it: whether the
/// newline was found — the reading goes on at the next line — or the
/// record ended first, the line not yet whole.
pub fn skip_line(reader: &mut impl BufRead) -> io::Result<bool> {
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return Ok(false);
        }
        match available.iter().position(|&byte| byte == b'\n') {
            Some(at) => {
                reader.consume(at + 1);
                return Ok(true);
            }
            None => {
                let length = available.len();
                reader.consume(length);
            }
        }
    }
}

/// Whether the JSON line `line` holds more than `VALUES_MOST` values,
/// counted by its punctuation outside strings before it is parsed.
pub fn too_many_values(line: &[u8]) -> bool {
    // Each value counted is a byte of the line: a line no longer than the
    // bound cannot hold more.
    if line.len() <= VALUES_MOST {
        return false;
    }
    let (mut values, mut string, mut escaped) = (0usize, false, false);
    for &byte in line {
        if string {
            match (escaped, byte) {
                (true, _) => escaped = false,
                (false, b'\\') => escaped = true,
                (false, b'"') => string = false,
                _ => {}
            }
            continue;
        }
        match byte {
            b'"' => string = true,
            b'{' | b'[' | b',' => {
                values += 1;
                if values > VALUES_MOST {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufReader;

    #[test]
    fn reads_lines_within_the_bound_whole() {
        let mut reader = BufReader::with_capacity(4, &b"{\"a\":1}\n{\"b\":\"x\"}"[..]);
        let mut line = Vec::new();
        assert_eq!(read_line(&mut reader, &mut line, 1).unwrap(), 8);
        assert_eq!(line, b"{\"a\":1}\n");
        line.clear();
        assert_eq!(read_line(&mut reader, &mut line, 2).unwrap(), 9);
        assert_eq!(line, b"{\"b\":\"x\"}");
    }

    #[test]
    fn refuses_a_longer_line() {
        let long = vec![b'x'; LINE_MOST + 10];
        let mut reader = BufReader::new(long.as_slice());
        let error = read_line(&mut reader, &mut Vec::new(), 7).unwrap_err();
        assert!(is_refused(&error) && error.to_string().contains("line 7"));
    }

    #[test]
    fn a_refused_line_says_whether_its_end_was_taken() {
        // Its newline in the chunk that refused it: taken, and the next line
        // read as any other — skipping again would lose it.
        let mut text = vec![b'x'; LINE_MOST + 10];
        text.extend_from_slice(b"\n{\"next\":1}\n");
        let mut reader = BufReader::with_capacity(LINE_MOST + 64, text.as_slice());
        assert!(matches!(read_bounded(&mut reader, &mut Vec::new()).unwrap(), Read::Refused { ended: true }));
        let mut line = Vec::new();
        assert!(matches!(read_bounded(&mut reader, &mut line).unwrap(), Read::Line(11)));
        assert_eq!(line, b"{\"next\":1}\n");
        // Its newline past the chunk that refused it: skipped to, then the
        // next line.
        let mut text = vec![b'x'; LINE_MOST + 100_000];
        text.extend_from_slice(b"\n{\"next\":1}\n");
        let mut reader = BufReader::with_capacity(1 << 16, text.as_slice());
        assert!(matches!(read_bounded(&mut reader, &mut Vec::new()).unwrap(), Read::Refused { ended: false }));
        assert!(skip_line(&mut reader).unwrap());
        let mut line = Vec::new();
        assert!(matches!(read_bounded(&mut reader, &mut line).unwrap(), Read::Line(11)));
        // One not yet whole: no end to skip to.
        let unended = vec![b'x'; LINE_MOST + 10];
        let mut reader = BufReader::with_capacity(1 << 16, unended.as_slice());
        assert!(matches!(read_bounded(&mut reader, &mut Vec::new()).unwrap(), Read::Refused { ended: false }));
        assert!(!skip_line(&mut reader).unwrap());
    }

    #[test]
    fn counts_values_outside_strings() {
        assert!(!too_many_values(br#"{"a":[1,2,3],"s":"{[,,,]}"}"#));
        let many = format!("[{}0]", "0,".repeat(VALUES_MOST + 1));
        assert!(too_many_values(many.as_bytes()));
        let quoted = format!("{{\"s\":\"{}\"}}", ",".repeat(VALUES_MOST + 1));
        assert!(!too_many_values(quoted.as_bytes()));
        let escaped = format!("{{\"s\":\"\\\"{}\"}}", ",".repeat(VALUES_MOST + 1));
        assert!(!too_many_values(escaped.as_bytes()));
    }
}
