//! Graphics protocols retain payload ranges and decode into bounded spools.
use super::terminal::{Image, Lines};
use crate::image::Header;
use crate::spool::{Bytes, Spool};
use base64::engine::general_purpose::STANDARD;
use base64::{DecodeError, Engine as _};
use std::io::{self, Read, Write};

fn separator(bytes: &Bytes, byte: u8) -> io::Result<Option<usize>> {
    let mut offset = 0;
    for chunk in bytes.chunks() {
        let chunk = chunk?;
        if let Some(at) = memchr::memchr(byte, &chunk) {
            return Ok(Some(offset + at));
        }
        offset += chunk.len();
    }
    Ok(None)
}
fn pairs(bytes: &Bytes, separator: u8, mut consume: impl FnMut(Bytes) -> io::Result<()>) -> io::Result<()> {
    let mut offset = 0;
    let mut start = 0;
    for chunk in bytes.chunks() {
        let chunk = chunk?;
        for at in memchr::memchr_iter(separator, &chunk) {
            consume(bytes.slice(start..offset + at))?;
            start = offset + at + 1;
        }
        offset += chunk.len();
    }
    consume(bytes.slice(start..bytes.len()))
}
fn equal(bytes: &Bytes, value: &[u8]) -> io::Result<bool> {
    Ok(bytes.len() == value.len() && bytes.to_vec()? == value)
}
fn trim(bytes: &Bytes) -> io::Result<Bytes> {
    let mut first = None;
    let mut last = 0;
    let mut offset = 0;
    for chunk in bytes.chunks() {
        let chunk = chunk?;
        if let Some(at) = chunk.iter().position(|b| !b.is_ascii_whitespace()) {
            first.get_or_insert(offset + at);
        }
        if let Some(at) = chunk.iter().rposition(|b| !b.is_ascii_whitespace()) {
            last = offset + at + 1;
        }
        offset += chunk.len();
    }
    Ok(bytes.slice(first.unwrap_or(last)..last))
}
pub(super) fn iterm<R: Read>(lines: &mut Lines<R>, rest: Bytes) -> io::Result<()> {
    let Some(colon) = separator(&rest, b':')? else {
        return lines.unreadable("an iTerm2 image without data".into());
    };
    let mut inline = false;
    pairs(&rest.slice(0..colon), b';', |pair| {
        inline |= equal(&pair, b"inline=1")?;
        Ok(())
    })?;
    if !inline {
        return Ok(());
    }
    image(lines, trim(&rest.slice(colon + 1..rest.len()))?, "an iTerm2 image whose base64 does not decode")
}
pub(super) fn kitty<R: Read>(lines: &mut Lines<R>, bytes: Bytes) -> io::Result<()> {
    let split = separator(&bytes, b';')?;
    let control = bytes.slice(0..split.unwrap_or(bytes.len()));
    let payload = bytes.slice(split.map_or(bytes.len(), |at| at + 1)..bytes.len());
    let mut format = false;
    let mut medium = true;
    let mut more = false;
    let mut action = true;
    pairs(&control, b',', |pair| {
        match pair.byte(0)? {
            Some(b'f') => format = equal(&pair, b"f=100")?,
            Some(b't') if pair.byte(1)? == Some(b'=') => medium = equal(&pair, b"t=d")?,
            Some(b'm') => more = equal(&pair, b"m=1")?,
            Some(b'a') if pair.byte(1)? == Some(b'=') => action = equal(&pair, b"a=t")? || equal(&pair, b"a=T")?,
            _ => {}
        }
        Ok(())
    })?;
    let continuing = lines.kitty_buffer().is_some();
    if !continuing && !action {
        return Ok(());
    }
    if !continuing && (!format || !medium) {
        return lines.unreadable("a kitty image that is not a PNG sent directly (only f=100, t=d is shown)".into());
    }
    let gathered = lines.kitty_buffer().get_or_insert_with(Spool::default);
    io::copy(&mut trim(&payload)?.reader(), gathered)?;
    if more {
        return Ok(());
    }
    let gathered = lines.kitty_buffer().take().expect("gathered").finish();
    image(lines, gathered, "a kitty image whose base64 does not decode")
}
fn image<R: Read>(lines: &mut Lines<R>, encoded: Bytes, reason: &str) -> io::Result<()> {
    let bytes = match decode(&encoded)? {
        Ok(bytes) => bytes,
        Err(error) => return lines.unreadable(format!("{reason}: {error}")),
    };
    match crate::image::header(bytes.reader())? {
        Some(Header::Shown { mime, width, height }) => lines.image(Image { mime, width, height, bytes }),
        Some(Header::Unreadable { format, reason }) => lines
            .unreadable(format!("an image of {} bytes whose {format} header cannot be read: {reason}", bytes.len())),
        Some(Header::Other(_)) | None => {
            lines.unreadable(format!("an image of {} bytes in a format other than PNG, JPEG, GIF or WebP", bytes.len()))
        }
    }
}

fn alphabet(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/')
}

/// The slice decoder checks a bad last symbol before earlier symbols when
/// length modulo four is one. Preserve that diagnostic priority during replay.
fn decode(bytes: &Bytes) -> io::Result<Result<Bytes, DecodeError>> {
    let length = bytes.len();
    let remainder = length % 4;
    if remainder == 1 {
        let last = bytes.byte(length - 1)?.expect("nonempty");
        if last != b'=' && !alphabet(last) {
            return Ok(Err(DecodeError::InvalidByte(length - 1, last)));
        }
    }
    let prefix = length.saturating_sub(remainder).saturating_sub(if remainder == 0 { 4 } else { 0 });
    let mut input = bytes.reader();
    let mut buffer = [0; crate::spool::BLOCK];
    let mut out = Spool::default();
    let mut at = 0;
    while at < prefix {
        let count = (prefix - at).min(buffer.len());
        input.read_exact(&mut buffer[..count])?;
        if let Some((offset, &byte)) = buffer[..count].iter().enumerate().find(|(_, byte)| !alphabet(**byte)) {
            return Ok(Err(DecodeError::InvalidByte(at + offset, byte)));
        }
        let decoded = STANDARD.decode(&buffer[..count]).map_err(io::Error::other)?;
        out.write_all(&decoded)?;
        at += count;
    }
    match STANDARD.decode(bytes.slice(prefix..length).to_vec()?) {
        Ok(decoded) => {
            out.write_all(&decoded)?;
            Ok(Ok(out.finish()))
        }
        Err(error) => Ok(Err(match error {
            DecodeError::InvalidByte(at, byte) => DecodeError::InvalidByte(prefix + at, byte),
            DecodeError::InvalidLastSymbol(at, byte) => DecodeError::InvalidLastSymbol(prefix + at, byte),
            DecodeError::InvalidLength(length) => DecodeError::InvalidLength(prefix + length),
            DecodeError::InvalidPadding => DecodeError::InvalidPadding,
        })),
    }
}
