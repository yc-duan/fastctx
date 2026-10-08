//! Streaming text selection. Unmarked logs follow `run` one line at a time,
//! so UTF-8 and native program output can share a file without mojibake.

use std::collections::BTreeMap;
use std::io;

use fastctx_platform::legacy_text::{CodePage, LegacyDecoder, code_pages};

use crate::encoding::{self, EncodingError, TextDecoder};

use super::text::Reader;

pub(super) enum Decoder {
    Explicit { decoder: TextDecoder, label: String, bom: usize },
    Auto(Auto),
}

impl Decoder {
    pub fn new(start: &[u8], label: Option<&str>) -> Result<Self, String> {
        let bom = start.starts_with(b"\xef\xbb\xbf")
            || start.starts_with(b"\xff\xfe")
            || start.starts_with(b"\xfe\xff")
            || start.starts_with(b"\x00\x00\xfe\xff");
        if label.is_some() || bom {
            // Detection only selects here; the streaming decoder validates
            // every byte, including a character split at the sniff boundary.
            let encoding = encoding::select(start, label).map_err(|error| error.to_string())?;
            return Ok(Self::Explicit {
                decoder: TextDecoder::new(encoding),
                label: encoding.label().into(),
                bom: encoding.bom_len(),
            });
        }
        Ok(Self::Auto(Auto::new().map_err(|error| error.to_string())?))
    }

    pub fn feed(&mut self, bytes: &[u8], end: bool, reader: &mut Reader) -> Result<(), String> {
        match self {
            Self::Explicit { decoder, .. } => {
                let text = decoder.feed(bytes, end).map_err(|error| error.to_string())?;
                reader.take(&text);
                Ok(())
            }
            Self::Auto(auto) => auto.feed(bytes, end, reader),
        }
    }

    pub fn facts(&self) -> String {
        match self {
            Self::Explicit { label, bom, .. } if label == "UTF-8" => {
                if *bom > 0 {
                    "UTF-8 with BOM; ".into()
                } else {
                    String::new()
                }
            }
            Self::Explicit { label, .. } => format!("decoded from {label}; "),
            Self::Auto(auto) => auto
                .used
                .iter()
                .map(|(name, count)| format!("{} decoded from {name}; ", super::plural(*count, "line", "lines")))
                .collect(),
        }
    }

    pub fn encoding(&self) -> Option<String> {
        match self {
            Self::Explicit { label, .. } => Some(label.clone()),
            Self::Auto(auto) if auto.used.is_empty() => Some("UTF-8".into()),
            // A mixed log has no single encoding a writer can preserve.
            Self::Auto(auto) if !auto.non_ascii_utf8 && auto.used.len() == 1 => auto.legacy_label.clone(),
            Self::Auto(_) => None,
        }
    }

    pub fn longest_facts(&self) -> String {
        match self {
            Self::Explicit { .. } => self.facts(),
            Self::Auto(auto) => {
                auto.pages.iter().map(|page| format!("{} lines decoded from {}; ", u64::MAX, page.name())).collect()
            }
        }
    }
}

pub(super) struct Auto {
    pages: Vec<CodePage>,
    candidates: Vec<Candidate>,
    offset: u64,
    bytes_in_line: u64,
    prefix: Vec<u8>,
    used: BTreeMap<String, u64>,
    non_ascii_utf8: bool,
    legacy_label: Option<String>,
}

impl Auto {
    fn new() -> io::Result<Self> {
        let mut auto = Self {
            pages: code_pages(),
            candidates: Vec::new(),
            offset: 0,
            bytes_in_line: 0,
            prefix: Vec::new(),
            used: BTreeMap::new(),
            non_ascii_utf8: false,
            legacy_label: None,
        };
        auto.reset()?;
        Ok(auto)
    }

    fn reset(&mut self) -> io::Result<()> {
        self.candidates = vec![Candidate::new(Codec::Utf8(TextDecoder::new(encoding::TextEncoding::utf8())))];
        for page in &self.pages {
            self.candidates.push(Candidate::new(Codec::Legacy(LegacyDecoder::new(*page)?)));
        }
        Ok(())
    }

    fn feed(&mut self, bytes: &[u8], end: bool, reader: &mut Reader) -> Result<(), String> {
        for part in bytes.split_inclusive(|&byte| byte == b'\n') {
            let newline = part.last() == Some(&b'\n');
            let content = if newline { &part[..part.len() - 1] } else { part };
            let keep = content.len().min(super::text::SNIFF - self.prefix.len());
            self.prefix.extend_from_slice(&content[..keep]);
            self.bytes_in_line += part.len() as u64;
            for candidate in &mut self.candidates {
                candidate.feed(content, newline, reader.room());
            }
            if newline {
                self.finish_line(reader, true)?;
            }
        }
        if end && self.bytes_in_line > 0 {
            for candidate in &mut self.candidates {
                candidate.feed(&[], true, reader.room());
            }
            self.finish_line(reader, false)?;
        }
        Ok(())
    }

    fn finish_line(&mut self, reader: &mut Reader, newline: bool) -> Result<(), String> {
        let Some(index) = self.candidates.iter().position(|candidate| candidate.failed.is_none()) else {
            let failures: Vec<String> = self
                .candidates
                .iter()
                .enumerate()
                .map(|(index, candidate)| {
                    let name = if index == 0 { "UTF-8".into() } else { self.pages[index - 1].name() };
                    format!("{name} at byte offset {}", self.offset + candidate.failed.expect("all failed"))
                })
                .collect();
            let complete = self.prefix.len() as u64 == self.bytes_in_line - u64::from(newline);
            let hint = match encoding::detect_prefix(&self.prefix, complete, None) {
                Err(EncodingError::Ambiguous { candidates, .. }) => {
                    format!("; prefix candidates: {}", candidates.join(", "))
                }
                _ => String::new(),
            };
            return Err(format!(
                "decoding failed: {}{hint}; use --encoding <label> to select another encoding",
                failures.join(", ")
            ));
        };
        let selected = &mut self.candidates[index];
        if index == 0 {
            self.non_ascii_utf8 |= selected.non_ascii;
        } else {
            let page = self.pages[index - 1];
            *self.used.entry(page.name()).or_default() += 1;
            self.legacy_label = page.label().map(str::to_owned);
        }
        reader.line(std::mem::take(&mut selected.text), selected.characters, selected.ends_with_cr, newline);
        self.offset += self.bytes_in_line;
        self.bytes_in_line = 0;
        self.prefix.clear();
        self.reset().map_err(|error| error.to_string())
    }
}

enum Codec {
    Utf8(TextDecoder),
    Legacy(LegacyDecoder),
}

struct Candidate {
    codec: Codec,
    text: String,
    kept: usize,
    characters: u64,
    ends_with_cr: bool,
    non_ascii: bool,
    failed: Option<u64>,
}

impl Candidate {
    fn new(codec: Codec) -> Self {
        Self { codec, text: String::new(), kept: 0, characters: 0, ends_with_cr: false, non_ascii: false, failed: None }
    }

    fn feed(&mut self, bytes: &[u8], end: bool, room: usize) {
        if self.failed.is_some() {
            return;
        }
        let decoded = match &mut self.codec {
            Codec::Utf8(decoder) => decoder.feed(bytes, end).map_err(|error| match error {
                EncodingError::Malformed { offset, .. } => offset as u64,
                _ => unreachable!("a selected UTF-8 decoder only reports malformed input"),
            }),
            Codec::Legacy(decoder) => decoder.feed(bytes, end),
        };
        match decoded {
            Ok(text) => {
                self.non_ascii |= !text.is_ascii();
                let characters = text.chars().count();
                let keep = characters.min(room.saturating_add(1).saturating_sub(self.kept));
                self.text.extend(text.chars().take(keep));
                self.kept += keep;
                self.characters += characters as u64;
                if !text.is_empty() {
                    self.ends_with_cr = text.ends_with('\r');
                }
            }
            Err(offset) => self.failed = Some(offset),
        }
    }
}
