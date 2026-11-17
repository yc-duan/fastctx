//! Length-prefixed byte frames. A reply is a one-byte outcome frame followed
//! by its payload frame; the payload's full 64 MiB remain available to callers.

use std::io::{self, Read, Write};

pub(super) const MAX: usize = 64 * 1024 * 1024;
pub(super) const TOO_LARGE: &str = "a worker frame exceeds 64 MiB";

pub(super) fn write(output: &mut impl Write, bytes: &[u8]) -> io::Result<()> {
    if bytes.len() > MAX {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, TOO_LARGE));
    }
    output.write_all(&(bytes.len() as u32).to_le_bytes())?;
    output.write_all(bytes)?;
    output.flush()
}

pub(super) enum Frame {
    End,
    Bytes(Vec<u8>),
    Oversized,
}

pub(super) fn read(input: &mut impl Read) -> io::Result<Frame> {
    let mut length = [0; 4];
    loop {
        match input.read(&mut length[..1]) {
            Ok(0) => return Ok(Frame::End),
            Ok(_) => break,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
    input.read_exact(&mut length[1..])?;
    let length = u32::from_le_bytes(length) as usize;
    if length > MAX {
        return Ok(Frame::Oversized);
    }
    let mut bytes = vec![0; length];
    input.read_exact(&mut bytes)?;
    Ok(Frame::Bytes(bytes))
}

pub(super) fn reply(output: &mut impl Write, result: Result<Vec<u8>, String>) -> io::Result<()> {
    let (outcome, payload) = match result {
        Ok(bytes) if bytes.len() <= MAX => (0, bytes),
        Ok(_) => (1, TOO_LARGE.as_bytes().to_vec()),
        Err(reason) if reason.len() <= MAX => (1, reason.into_bytes()),
        Err(_) => (1, TOO_LARGE.as_bytes().to_vec()),
    };
    write(output, &[outcome])?;
    write(output, &payload)
}

pub(super) fn answer(input: &mut impl Read) -> io::Result<Result<Vec<u8>, String>> {
    let outcome = match read(input)? {
        Frame::Bytes(bytes) if bytes.len() == 1 && bytes[0] <= 1 => bytes[0],
        Frame::Oversized => return Ok(Err(TOO_LARGE.to_string())),
        _ => return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "missing worker reply outcome")),
    };
    match read(input)? {
        Frame::Bytes(bytes) if outcome == 0 => Ok(Ok(bytes)),
        Frame::Bytes(bytes) => Ok(Err(String::from_utf8_lossy(&bytes).into_owned())),
        Frame::Oversized => Ok(Err(TOO_LARGE.to_string())),
        Frame::End => Err(io::Error::new(io::ErrorKind::UnexpectedEof, "missing worker reply payload")),
    }
}
