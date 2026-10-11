use crate::spool::Spool;
use std::io::{self, Read, Write};

use super::file::{Plan, describe_error};
use super::statistics::Statistics;
use super::{Codec, TextDecoder, TextEncoding, bom, explicit};

struct Validation {
    decoder: TextDecoder,
    valid: bool,
    printable: u64,
    other: u64,
}

impl Validation {
    fn new(label: &str) -> Self {
        Self {
            decoder: TextDecoder::new(explicit(label, &[]).expect("known codec")),
            valid: true,
            printable: 0,
            other: 0,
        }
    }

    fn feed(&mut self, bytes: &[u8], end: bool) {
        if !self.valid {
            return;
        }
        match self.decoder.feed(bytes, end) {
            Ok(text) => {
                for c in text.chars() {
                    if printable(c) {
                        self.printable += 1;
                    } else {
                        self.other += 1;
                    }
                }
            }
            Err(_) => self.valid = false,
        }
    }
}

pub(super) fn printable(c: char) -> bool {
    (!c.is_control() || matches!(c, '\n' | '\r' | '\t'))
        && !(0xfdd0..=0xfdef).contains(&(c as u32))
        && (c as u32 & 0xffff) < 0xfffe
}

pub(super) fn inspect(reader: &mut impl Read, selection: Option<&str>) -> Result<Plan, String> {
    let mut first = [0; 4];
    let mut length = 0;
    while length < first.len() {
        match reader.read(&mut first[length..]) {
            Ok(0) => break,
            Ok(n) => length += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.to_string()),
        }
    }
    let first = &first[..length];
    if let Some(selection) = selection {
        let codecs: Vec<_> = selection
            .split(',')
            .map(|label| explicit(label.trim(), first))
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string())?;
        if codecs.len() == 1 {
            return Ok(Plan::fixed(codecs[0], "explicit --encoding"));
        }
        if codecs.iter().any(|codec| codec.unit() != 1) {
            return Err("An encoding list requires codecs whose line separator is the byte LF; select UTF-16 or UTF-32 on its own".into());
        }
        return Ok(Plan {
            fixed: None,
            choices: codecs,
            basis: "explicit per-line encoding order",
            ordered: true,
            legacy: Vec::new(),
        });
    }
    if let Some(codec) = bom(first) {
        return Ok(Plan::fixed(codec, "BOM"));
    }

    let mut statistics = Statistics::new();
    let mut line = Spool::default();
    let mut utf8 = Validation::new("utf-8");
    let mut le = Validation::new("utf-16le");
    let mut be = Validation::new("utf-16be");
    let mut iso = Validation::new("iso-2022-jp");
    let mut zeroes = [0u64; 2];
    let mut first_zero = None;
    let mut offset = 0u64;
    let mut escape = [0u8; 3];
    let mut iso_offset = None;
    let mut probe = |bytes: &[u8], end| -> Result<(), String> {
        utf8.feed(bytes, end);
        for piece in bytes.split_inclusive(|byte| *byte == b'\n') {
            line.write_all(piece).map_err(|e| e.to_string())?;
            if piece.ends_with(b"\n") {
                inspect_line(&mut line, &mut statistics)?;
            }
        }
        if end {
            inspect_line(&mut line, &mut statistics)?;
            statistics.feed(&[], true);
        }
        le.feed(bytes, end);
        be.feed(bytes, end);
        iso.feed(bytes, end);
        for &byte in bytes {
            if byte == 0 {
                zeroes[(offset % 2) as usize] += 1;
                first_zero.get_or_insert(offset);
            }
            escape.rotate_left(1);
            escape[2] = byte;
            if matches!(&escape, b"\x1b$B" | b"\x1b$@" | b"\x1b(B" | b"\x1b(J" | b"\x1b(I") {
                iso_offset.get_or_insert(offset.saturating_sub(2));
            }
            offset += 1;
        }
        Ok(())
    };
    probe(first, false)?;
    let mut buffer = [0; 64 * 1024];
    loop {
        let n = match reader.read(&mut buffer) {
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.to_string()),
        };
        probe(&buffer[..n], n == 0)?;
        if n == 0 {
            break;
        }
    }
    if let Some(offset) = first_zero {
        // A positional pattern needs at least two observations. Requiring all
        // NULs on that parity avoids silently accepting interleaved binary data.
        let chosen = if zeroes[0] == 0 && zeroes[1] >= 2 && le.valid && le.printable > le.other {
            Some("utf-16le")
        } else if zeroes[1] == 0 && zeroes[0] >= 2 && be.valid && be.printable > be.other {
            Some("utf-16be")
        } else {
            None
        };
        return chosen.map(|label| Plan::fixed(explicit(label, &[]).expect("known codec"), "NUL byte pattern and strict UTF-16 validation"))
            .ok_or_else(|| format!("the file has NUL bytes from byte {offset} and is not clearly UTF-16, so it is taken as binary; if it is text, view it with --encoding to name its encoding"));
    }
    if iso_offset.is_some() && iso.valid {
        return Ok(Plan::fixed(
            explicit("iso-2022-jp", &[]).expect("known codec"),
            "ISO-2022-JP escape sequences and strict validation",
        ));
    }
    if utf8.valid && iso_offset.is_none() {
        return Ok(Plan::fixed(TextEncoding::utf8(), "UTF-8"));
    }
    Ok(Plan {
        fixed: None,
        choices: Vec::new(),
        basis: super::statistics::BASIS,
        ordered: false,
        legacy: statistics.leaders(),
    })
}

fn inspect_line(line: &mut Spool, statistics: &mut Statistics) -> Result<(), String> {
    if let Some(bytes) = line.as_slice() {
        if std::str::from_utf8(bytes).is_err() {
            statistics.feed(bytes, false);
        }
        line.clear();
        return Ok(());
    }
    let bytes = std::mem::take(line).finish();
    let mut decoder = TextDecoder::new(TextEncoding::utf8());
    let mut valid = true;
    for chunk in bytes.chunks() {
        let chunk = chunk.map_err(|e| e.to_string())?;
        valid = valid && decoder.feed(&chunk, false).is_ok();
    }
    valid = valid && decoder.feed(&[], true).is_ok();
    if !valid {
        for chunk in bytes.chunks() {
            statistics.feed(&chunk.map_err(|e| e.to_string())?, false);
        }
    }
    Ok(())
}

impl TextEncoding {
    pub(super) fn unit(self) -> usize {
        match self.codec {
            Codec::Utf32Le | Codec::Utf32Be => 4,
            Codec::Rs(enc) if enc == encoding_rs::UTF_16LE || enc == encoding_rs::UTF_16BE => 2,
            _ => 1,
        }
    }

    pub(crate) fn is_stateful(self) -> bool {
        self.codec == Codec::Rs(encoding_rs::ISO_2022_JP)
    }

    pub(super) fn without_bom(self) -> Self {
        Self { bom_len: 0, ..self }
    }

    pub(super) fn decode_at(self, bytes: &[u8], offset: usize) -> Result<String, String> {
        self.without_bom().decode(bytes).map_err(|error| describe_error(&error, bytes, offset))
    }
}
