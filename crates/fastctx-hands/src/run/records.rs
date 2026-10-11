//! Sequential row records refer to payload ranges instead of retaining payloads.

use super::terminal::{Image, Part};
use crate::spool::{Bytes, Spool};
use std::io::{self, Read, Write};

pub(super) struct Record {
    pub kind: u64,
    pub a: u64,
    pub b: u64,
    pub c: u64,
    pub payload: Bytes,
}

pub(super) fn write(out: &mut Spool, kind: u64, a: u64, b: u64, c: u64, payload: &Bytes) -> io::Result<()> {
    for word in [kind, a, b, c, payload.len() as u64] {
        out.write_all(&word.to_le_bytes())?;
    }
    io::copy(&mut payload.reader(), out)?;
    Ok(())
}
pub(super) fn small(out: &mut Spool, kind: u64, a: u64, b: u64, c: u64, payload: &[u8]) -> io::Result<()> {
    for word in [kind, a, b, c, payload.len() as u64] {
        out.write_all(&word.to_le_bytes())?;
    }
    out.write_all(payload)
}
pub(super) struct Records {
    data: Bytes,
    at: usize,
}
impl Records {
    pub fn new(data: Bytes) -> Self {
        Self { data, at: 0 }
    }
}
impl Iterator for Records {
    type Item = io::Result<Record>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.at == self.data.len() {
            return None;
        }
        let read = || -> io::Result<Record> {
            let mut header = [0; 40];
            self.data.slice(self.at..self.at + 40).reader().read_exact(&mut header)?;
            let words: [u64; 5] =
                std::array::from_fn(|i| u64::from_le_bytes(header[i * 8..(i + 1) * 8].try_into().expect("word")));
            let start = self.at + 40;
            let end = start.checked_add(words[4] as usize).ok_or_else(|| io::Error::other("record length overflow"))?;
            Ok(Record { kind: words[0], a: words[1], b: words[2], c: words[3], payload: self.data.slice(start..end) })
        };
        let result = read();
        if let Ok(record) = &result {
            self.at += 40 + record.payload.len();
        } else {
            self.at = self.data.len();
        }
        Some(result)
    }
}

#[derive(Debug, Clone)]
pub struct Parts<'a>(Data<'a>);

#[derive(Debug, Clone)]
enum Data<'a> {
    Text(std::borrow::Cow<'a, str>),
    Records(Bytes),
}
impl<'a> Parts<'a> {
    /// Stream text without collecting a potentially enormous terminal row.
    /// Images remain identifiable in a text viewer through a size label.
    pub fn write_text(&self, out: &mut impl Write) -> io::Result<()> {
        match &self.0 {
            Data::Text(text) => out.write_all(text.as_bytes()),
            Data::Records(bytes) => {
                for record in Records::new(bytes.clone()) {
                    let record = record?;
                    match record.kind {
                        0 | 3 => {
                            io::copy(&mut record.payload.reader(), out)?;
                        }
                        1 => {
                            let spaces = [b' '; crate::spool::BLOCK];
                            let mut remaining = record.a;
                            while remaining > 0 {
                                let count = remaining.min(spaces.len() as u64) as usize;
                                out.write_all(&spaces[..count])?;
                                remaining -= count as u64;
                            }
                        }
                        2 => write!(out, "[image {}x{}]", record.a, record.b)?,
                        _ => return Err(io::Error::other("invalid terminal part")),
                    }
                }
                Ok(())
            }
        }
    }

    pub(super) fn from_text(text: std::borrow::Cow<'a, str>) -> Self {
        Self(Data::Text(text))
    }
    pub(super) fn from_records(bytes: Bytes) -> Self {
        Self(Data::Records(bytes))
    }
    pub fn text(&self) -> Option<&str> {
        match &self.0 {
            Data::Text(text) => Some(text),
            Data::Records(_) => None,
        }
    }
    pub fn iter(&self) -> impl Iterator<Item = io::Result<Part>> + '_ {
        let (text, records) = match &self.0 {
            Data::Text(text) => (Some(Ok(Part::Text(text.to_string()))), None),
            Data::Records(bytes) => (None, Some(Records::new(bytes.clone()))),
        };
        text.into_iter().chain(records.into_iter().flatten().map(|record| {
            let record = record?;
            Ok(match record.kind {
                0 => Part::Text(record.payload.text()?),
                1 => Part::Spaces(record.a as usize),
                2 => Part::Image(Image {
                    mime: mime(record.c)?,
                    width: record.a as u32,
                    height: record.b as u32,
                    bytes: record.payload,
                }),
                3 => Part::Unreadable(record.payload.text()?),
                _ => return Err(io::Error::other("invalid terminal part")),
            })
        }))
    }
    pub(super) fn write(out: &mut Spool, part: &Part) -> io::Result<()> {
        match part {
            Part::Text(text) => small(out, 0, 0, 0, 0, text.as_bytes()),
            Part::Spaces(count) => small(out, 1, *count as u64, 0, 0, &[]),
            Part::Image(image) => {
                write(out, 2, u64::from(image.width), u64::from(image.height), mime_id(image.mime), &image.bytes)
            }
            Part::Unreadable(reason) => small(out, 3, 0, 0, 0, reason.as_bytes()),
        }
    }
}
pub(super) fn mime_id(mime: &str) -> u64 {
    match mime {
        "image/png" => 0,
        "image/jpeg" => 1,
        "image/gif" => 2,
        "image/webp" => 3,
        _ => unreachable!("supported image"),
    }
}
pub(super) fn mime(id: u64) -> io::Result<&'static str> {
    match id {
        0 => Ok("image/png"),
        1 => Ok("image/jpeg"),
        2 => Ok("image/gif"),
        3 => Ok("image/webp"),
        _ => Err(io::Error::other("invalid image format")),
    }
}
