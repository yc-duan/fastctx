//! ASCII rows without horizontal edits can be emitted in blocks, including literals.

use crate::spool::{BLOCK, Bytes};
use std::io;

/// Where emitted text goes; without one, the walk only checks the span.
type Consume<'a> = &'a mut dyn FnMut(&str) -> io::Result<()>;

#[derive(Clone, Copy)]
enum State {
    Normal,
    Escape(usize),
    OtherEscape(usize),
    Csi { start: usize, intermediate: bool, sgr: bool, short: [u8; 5], length: usize },
}

struct Output<'a> {
    text: String,
    consume: Option<Consume<'a>>,
}
impl Output<'_> {
    fn byte(&mut self, byte: u8, literal: bool) -> io::Result<()> {
        if self.consume.is_none() {
            return Ok(());
        }
        if byte.is_ascii_control() && (byte != b'\t' || literal) {
            const HEX: &[u8] = b"0123456789ABCDEF";
            self.text.push_str(r"\x");
            self.text.push(HEX[(byte >> 4) as usize] as char);
            self.text.push(HEX[(byte & 15) as usize] as char);
        } else {
            self.text.push(byte as char);
        }
        if self.text.len() >= BLOCK {
            self.flush()?;
        }
        Ok(())
    }
    fn literal(&mut self, bytes: &Bytes, start: usize, end: usize) -> io::Result<()> {
        if self.consume.is_some() {
            for chunk in bytes.slice(start..end).chunks() {
                for byte in chunk? {
                    self.byte(byte, true)?;
                }
            }
        }
        Ok(())
    }
    fn flush(&mut self) -> io::Result<()> {
        if !self.text.is_empty() {
            self.consume.as_mut().expect("emitting")(&self.text)?;
            self.text.clear();
        }
        Ok(())
    }
}

pub(super) fn check(bytes: &Bytes, literal: bool) -> io::Result<bool> {
    walk(bytes, literal, None)
}

pub(super) fn render(bytes: &Bytes, literal: bool, mut consume: impl FnMut(&str) -> io::Result<()>) -> io::Result<()> {
    if !walk(bytes, literal, Some(&mut consume))? {
        return Err(io::Error::other("a validated linear terminal span changed"));
    }
    Ok(())
}

fn walk(bytes: &Bytes, literal: bool, consume: Option<Consume<'_>>) -> io::Result<bool> {
    let mut output = Output { text: String::new(), consume };
    let mut state = State::Normal;
    let mut offset = 0;
    for chunk in bytes.chunks() {
        for byte in chunk? {
            if !byte.is_ascii() {
                return Ok(false);
            }
            if literal {
                output.byte(byte, true)?;
                offset += 1;
                continue;
            }
            loop {
                state = match state {
                    State::Normal => match byte {
                        b'\r' | 8 => return Ok(false),
                        0x1b => State::Escape(offset),
                        _ => {
                            output.byte(byte, false)?;
                            State::Normal
                        }
                    },
                    State::Escape(start) => match byte {
                        b'[' => State::Csi { start, intermediate: false, sgr: true, short: [0; 5], length: 0 },
                        b'7' | b'8' => return Ok(false),
                        0x20..=0x2f => State::OtherEscape(start),
                        0x30..=0x7e => State::Normal,
                        _ => {
                            output.literal(bytes, start, offset)?;
                            state = State::Normal;
                            continue;
                        }
                    },
                    State::OtherEscape(start) => match byte {
                        0x20..=0x2f => State::OtherEscape(start),
                        0x30..=0x7e => State::Normal,
                        _ => {
                            output.literal(bytes, start, offset)?;
                            state = State::Normal;
                            continue;
                        }
                    },
                    State::Csi { start, mut intermediate, mut sgr, mut short, mut length } => {
                        match byte {
                            0x30..=0x3f if !intermediate => {}
                            0x20..=0x2f => intermediate = true,
                            0x40..=0x7e => {
                                if !(byte == b'm' && sgr
                                    || matches!(byte, b'h' | b'l')
                                        && length <= short.len()
                                        && matches!(&short[..length], b"?25" | b"?12" | b"?2004"))
                                {
                                    return Ok(false);
                                }
                                state = State::Normal;
                                break;
                            }
                            _ => {
                                output.literal(bytes, start, offset)?;
                                state = State::Normal;
                                continue;
                            }
                        }
                        if length < short.len() {
                            short[length] = byte;
                        }
                        length = (length + 1).min(short.len() + 1);
                        sgr &= byte.is_ascii_digit() || matches!(byte, b';' | b':');
                        State::Csi { start, intermediate, sgr, short, length }
                    }
                };
                break;
            }
            offset += 1;
        }
    }
    match state {
        State::Normal => {}
        State::Escape(start) | State::OtherEscape(start) | State::Csi { start, .. } => {
            output.literal(bytes, start, offset)?;
        }
    }
    output.flush()?;
    Ok(true)
}
