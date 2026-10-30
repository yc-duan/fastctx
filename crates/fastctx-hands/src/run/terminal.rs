//! Terminal rows are decoded and laid out through replayable temporary ranges.

use super::display::Display;
use super::records::{self, Parts, Records};
use super::view_set::ViewSet;
use crate::encoding::{TextDecoder, TextEncoding};
use crate::spool::{Bytes, Spool};
use fastctx_platform::legacy_text::{CodePage, LegacyDecoder, code_pages};
use serde::{Deserialize, Serialize};
use std::io::{self, Read, Write};

#[derive(Debug, Clone)]
pub struct Line<'a> {
    pub number: u64,
    pub through: u64,
    pub offset: usize,
    pub characters: usize,
    pub images: u64,
    pub graphics: u64,
    pub parts: Parts<'a>,
    pub decoding: Decoding,
    pub view: u64,
    pub newline: bool,
}
#[derive(Debug, Clone)]
pub enum Part {
    Text(String),
    Spaces(usize),
    Image(Image),
    Unreadable(String),
}
#[derive(Debug, Clone)]
pub struct Image {
    pub mime: &'static str,
    pub width: u32,
    pub height: u32,
    pub(crate) bytes: Bytes,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Decoding {
    pub legacy: Vec<String>,
    pub escaped: bool,
}

pub const VIEW_MARK: &str = "\x1b]7701;fastctx;view\x07";
pub const VIEW_END: &str = "\x1b]7701;fastctx;end\x07";
/// Written by one of the World's commands that failed, before the command
/// as it was given and a closing BEL: the run's answer names it whatever
/// the script's exit status (`world-run.md`).
pub const FAILED_MARK: &str = "\x1b]7701;fastctx;failed;";
/// The most failed commands one answer names; the most bytes of each.
const FAILED_KEPT: usize = 100;
const FAILED_BYTES: usize = 300;

const TEXT: u64 = 0;
const LITERAL: u64 = 1;
const BOUNDARY: u64 = 2;
const VIEW: u64 = 3;
const IMAGE: u64 = 4;
const UNREADABLE: u64 = 5;

#[derive(Clone, Copy)]
enum State {
    Normal,
    Escape,
    String { apc: bool, escape: bool },
}

pub struct Lines<R> {
    input: R,
    buffer: Vec<u8>,
    start: usize,
    end: usize,
    number: u64,
    line_number: u64,
    raw_newlines: u64,
    raw_unterminated: bool,
    state: State,
    pending: Spool,
    pending_content: bool,
    text: Spool,
    kitty: Option<Spool>,
    finished: bool,
    input_bytes: u64,
    view: u64,
    line_view: u64,
    next_view: u64,
    closed_views: ViewSet,
    queued: Spool,
    ready: Option<Queue>,
    tracker: super::unfinished::Tracker,
    string_start: u64,
    matches: Option<Vec<(std::ops::Range<u64>, u64)>>,
    text_tags: Vec<(std::ops::Range<usize>, u64)>,
    tagged_records: Vec<Vec<(std::ops::Range<usize>, u64)>>,
    failed: Vec<String>,
}

impl<R: Read> Lines<R> {
    /// Original bytes consumed, excluding input buffered for later rows.
    pub fn position(&self) -> u64 {
        self.input_bytes - (self.end - self.start) as u64
    }

    /// Whether the next original row starts inside a literal view span.
    pub fn in_view(&self) -> bool {
        self.view > 0
    }

    pub fn new(input: R) -> Self {
        Self {
            input,
            buffer: vec![0; crate::spool::BLOCK],
            start: 0,
            end: 0,
            number: 0,
            line_number: 1,
            raw_newlines: 0,
            raw_unterminated: false,
            state: State::Normal,
            pending: Spool::default(),
            pending_content: false,
            text: Spool::default(),
            kitty: None,
            finished: false,
            input_bytes: 0,
            view: 0,
            line_view: 0,
            next_view: 1,
            closed_views: ViewSet::default(),
            queued: Spool::default(),
            ready: None,
            tracker: Default::default(),
            string_start: 0,
            matches: None,
            text_tags: Vec::new(),
            tagged_records: Vec::new(),
            failed: Vec::new(),
        }
    }
    pub(crate) fn tagged_at(input: R, raw_start: u64, ranges: Vec<(std::ops::Range<u64>, u64)>) -> Self {
        let mut lines = Self::new(input);
        lines.matches = Some(ranges);
        lines.input_bytes = raw_start;
        lines
    }

    fn append_text(&mut self, bytes: &[u8], source: u64) -> io::Result<()> {
        if let Some(ranges) = &self.matches {
            let end = source + bytes.len() as u64;
            let first = ranges.partition_point(|(range, _)| range.end <= source);
            for (range, tag) in ranges[first..].iter().take_while(|(range, _)| range.start < end) {
                self.text_tags.push((
                    self.text.len() as usize + (range.start.max(source) - source) as usize
                        ..self.text.len() as usize + (range.end.min(end) - source) as usize,
                    *tag,
                ));
            }
        }
        self.text.write_all(bytes)
    }

    pub fn next_line(&mut self) -> io::Result<Option<Line<'_>>> {
        let mut sequence = Spool::default();
        loop {
            if let Some(ready) = self.ready.as_mut() {
                if let Some(line) = ready.next()? {
                    return Ok(Some(line));
                }
                self.ready = None;
            }
            if self.queued.len() > 0 {
                self.ready = Some(Queue::new(std::mem::take(&mut self.queued).finish()));
                continue;
            }
            if self.start == self.end {
                if self.finished {
                    return Ok(None);
                }
                let read = self.input.read(&mut self.buffer)?;
                self.input_bytes += read as u64;
                if read == 0 {
                    self.finished = true;
                    self.finish(std::mem::take(&mut sequence).finish())?;
                    continue;
                }
                self.start = 0;
                self.end = read;
                self.tracker.feed(&self.buffer[..read]);
            }
            if matches!(self.state, State::Normal)
                && self.text.len() == 0
                && self.pending.len() == 0
                && let Some(length) = memchr::memchr(b'\n', &self.buffer[self.start..self.end])
                && let Some(row) = super::plain::text(&self.buffer[self.start..self.start + length])
                && self.matches.as_ref().is_none_or(|ranges| {
                    let start = self.position();
                    let end = start + length as u64 + 1;
                    let at = ranges.partition_point(|(range, _)| range.end <= start);
                    ranges.get(at).is_none_or(|(range, _)| range.start >= end)
                })
            {
                let start = self.start;
                self.start += length + 1;
                self.raw_newlines += 1;
                self.raw_unterminated = false;
                let number = self.line_number;
                self.number += 1;
                self.line_number = self.number + 1;
                self.line_view = self.view;
                let text = match row {
                    super::plain::Row::Original(length) => std::borrow::Cow::Borrowed(
                        std::str::from_utf8(&self.buffer[start..start + length]).expect("validated row"),
                    ),
                    super::plain::Row::Rendered(text) => std::borrow::Cow::Owned(text),
                };
                return Ok(Some(Line {
                    number,
                    through: self.number,
                    offset: 0,
                    characters: text.chars().count(),
                    images: 0,
                    graphics: 0,
                    parts: Parts::from_text(text),
                    decoding: Decoding::default(),
                    view: if self.matches.is_some() { 0 } else { self.view },
                    newline: true,
                }));
            }
            let available = &self.buffer[self.start..self.end];
            let span = match self.state {
                State::Normal => memchr::memchr2(b'\n', 0x1b, available).unwrap_or(available.len()),
                State::String { apc, escape: false } => {
                    let end = memchr::memchr3(0x1b, 0x18, 0x1a, available).unwrap_or(available.len());
                    if apc { end } else { memchr::memchr(0x07, &available[..end]).unwrap_or(end) }
                }
                _ => 0,
            };
            if span > 0 {
                let bytes = &available[..span];
                self.raw_unterminated = bytes[span - 1] != b'\n';
                self.raw_newlines += memchr::memchr_iter(b'\n', bytes).count() as u64;
                if matches!(self.state, State::Normal) {
                    let position = self.position();
                    if self.matches.is_some() {
                        let bytes = bytes.to_vec();
                        self.append_text(&bytes, position)?;
                    } else {
                        self.text.write_all(bytes)?;
                    }
                } else {
                    sequence.write_all(bytes)?;
                }
                self.start += span;
                continue;
            }
            let byte = self.buffer[self.start];
            self.start += 1;
            self.raw_unterminated = byte != b'\n';
            self.raw_newlines += u64::from(byte == b'\n');
            match self.state {
                State::Normal => match byte {
                    b'\n' => self.end_line(true)?,
                    0x1b => {
                        self.string_start = self.position() - 1;
                        self.string_start = self.position() - 1;
                        self.state = State::Escape;
                    }
                    _ => {
                        self.append_text(&[byte], self.position() - 1)?;
                    }
                },
                State::Escape => match byte {
                    b']' => self.state = State::String { apc: false, escape: false },
                    b'_' => self.state = State::String { apc: true, escape: false },
                    0x1b => {
                        self.append_text(&[byte], self.position() - 1)?;
                        self.string_start = self.position() - 1;
                    }
                    _ => {
                        self.append_text(&[0x1b], self.position() - 2)?;
                        self.state = State::Normal;
                        if byte == b'\n' {
                            self.end_line(true)?;
                        } else {
                            self.append_text(&[byte], self.position() - 1)?;
                        }
                    }
                },
                State::String { apc, escape } => match byte {
                    byte if escape && byte != b'\\' => {
                        self.literal_string(apc, std::mem::take(&mut sequence).finish(), false)?;
                        self.start -= 1;
                        self.raw_newlines -= u64::from(byte == b'\n');
                        self.state = State::Escape;
                    }
                    0x18 | 0x1a => {
                        self.literal_string(apc, std::mem::take(&mut sequence).finish(), false)?;
                        self.append_text(&[byte], self.position() - 1)?;
                        self.state = State::Normal;
                    }
                    0x07 if !apc => {
                        self.close_string(apc, std::mem::take(&mut sequence).finish())?;
                        self.state = State::Normal;
                    }
                    b'\\' if escape => {
                        self.close_string(apc, std::mem::take(&mut sequence).finish())?;
                        self.state = State::Normal;
                    }
                    0x1b => self.state = State::String { apc, escape: true },
                    _ => {
                        sequence.write_all(&[byte])?;
                        self.state = State::String { apc, escape: false };
                    }
                },
            }
        }
    }
    pub fn raw_lines(&self) -> u64 {
        self.raw_newlines + u64::from(self.raw_unterminated)
    }
    pub(super) fn closed_views(&self) -> &ViewSet {
        &self.closed_views
    }
    /// The World's commands that said they failed, as they were given.
    pub(super) fn failed(&self) -> &[String] {
        &self.failed
    }
    pub(super) fn unfinished(&self) -> Option<super::unfinished::Unfinished> {
        self.tracker.unfinished()
    }

    fn finish(&mut self, sequence: Bytes) -> io::Result<()> {
        if let State::String { apc, escape } = self.state {
            let prefix = sequence.slice(0..sequence.len().min(10)).to_vec()?;
            if (apc && prefix.starts_with(b"G")) || (!apc && prefix.starts_with(b"1337;File=")) {
                let what = if apc { "kitty" } else { "OSC" };
                self.unreadable(format!(
                    "an image ({what} sequence) cut off before its end, {} bytes",
                    sequence.len()
                ))?;
            }
            self.literal_string(apc, sequence, escape)?;
        } else if matches!(self.state, State::Escape) {
            self.append_text(&[0x1b], self.position() - 1)?;
        }
        if self.kitty.take().is_some() {
            self.unreadable("a kitty image whose last chunk never came".into())?;
        }
        if self.text.len() != 0 || self.pending_content {
            self.end_line(false)?;
        }
        Ok(())
    }
    fn literal_string(&mut self, apc: bool, sequence: Bytes, escape: bool) -> io::Result<()> {
        self.flush_text()?;
        let mut literal = Spool::default();
        literal.write_all(&[0x1b, if apc { b'_' } else { b']' }])?;
        let mut source = self.string_start;
        for bytes in sequence.chunks() {
            for piece in bytes?.split_inclusive(|b| *b == b'\n') {
                let newline = piece.ends_with(b"\n");
                literal.write_all(&piece[..piece.len() - usize::from(newline)])?;
                if newline {
                    source += self.literal(std::mem::take(&mut literal).finish(), source)? + 1;
                    self.end_line(true)?;
                }
            }
        }
        if escape {
            literal.write_all(&[0x1b])?;
        }
        self.literal(literal.finish(), source)?;
        Ok(())
    }
    fn literal(&mut self, bytes: Bytes, source: u64) -> io::Result<u64> {
        if !bytes.is_empty() {
            let mut tags = Vec::new();
            if let Some(ranges) = &self.matches {
                let end = source + bytes.len() as u64;
                let first = ranges.partition_point(|(range, _)| range.end <= source);
                for (range, tag) in ranges[first..].iter().take_while(|(range, _)| range.start < end) {
                    tags.push((
                        (range.start.max(source) - source) as usize..(range.end.min(end) - source) as usize,
                        *tag,
                    ));
                }
            }
            let id = self.save_tags(tags);
            records::write(&mut self.pending, LITERAL, id, 0, 0, &bytes)?;
            self.pending_content = true;
        }
        Ok(bytes.len() as u64)
    }
    fn save_tags(&mut self, tags: Vec<(std::ops::Range<usize>, u64)>) -> u64 {
        if tags.is_empty() {
            return 0;
        }
        self.tagged_records.push(tags);
        self.tagged_records.len() as u64
    }
    fn close_string(&mut self, apc: bool, body: Bytes) -> io::Result<()> {
        self.flush_text()?;
        records::small(&mut self.pending, BOUNDARY, 0, 0, 0, &[])?;
        let prefix = body.slice(0..body.len().min(VIEW_MARK.len())).to_vec()?;
        if apc && prefix.starts_with(b"G") {
            super::graphics::kitty(self, body.slice(1..body.len()))?;
        } else if !apc && prefix.starts_with(b"1337;File=") {
            super::graphics::iterm(self, body.slice(10..body.len()))?;
        } else if !apc && prefix == VIEW_MARK.as_bytes()[2..VIEW_MARK.len() - 1] {
            self.change_view(true)?;
        } else if !apc && prefix == VIEW_END.as_bytes()[2..VIEW_END.len() - 1] {
            self.change_view(false)?;
        } else if !apc && self.failed.len() < FAILED_KEPT {
            let mark = &FAILED_MARK.as_bytes()[2..];
            if body.len() > mark.len() && body.slice(0..mark.len()).to_vec()? == mark {
                let named = body.slice(mark.len()..body.len().min(mark.len() + FAILED_BYTES)).to_vec()?;
                self.failed.push(String::from_utf8_lossy(&named).into_owned());
            }
        }
        for bytes in body.chunks() {
            self.number += memchr::memchr_iter(b'\n', &bytes?).count() as u64;
        }
        Ok(())
    }
    fn change_view(&mut self, begin: bool) -> io::Result<()> {
        let view = if begin {
            let id = self.next_view;
            self.next_view += 1;
            id
        } else {
            if self.view != 0 {
                self.closed_views.insert(self.view)?;
            }
            0
        };
        self.flush_text()?;
        records::small(&mut self.pending, VIEW, view, 0, 0, &[])?;
        self.view = view;
        Ok(())
    }
    pub(super) fn kitty_buffer(&mut self) -> &mut Option<Spool> {
        &mut self.kitty
    }
    pub(super) fn image(&mut self, image: Image) -> io::Result<()> {
        self.flush_text()?;
        records::write(
            &mut self.pending,
            IMAGE,
            u64::from(image.width),
            u64::from(image.height),
            records::mime_id(image.mime),
            &image.bytes,
        )?;
        self.pending_content = true;
        Ok(())
    }
    pub(super) fn unreadable(&mut self, reason: String) -> io::Result<()> {
        self.flush_text()?;
        records::small(&mut self.pending, UNREADABLE, 0, 0, 0, reason.as_bytes())?;
        self.pending_content = true;
        Ok(())
    }
    fn flush_text(&mut self) -> io::Result<()> {
        if self.text.len() != 0 {
            let tags = std::mem::take(&mut self.text_tags);
            let id = self.save_tags(tags);
            records::write(&mut self.pending, TEXT, id, 0, 0, &std::mem::take(&mut self.text).finish())?;
            self.pending_content = true;
        }
        Ok(())
    }
    fn end_line(&mut self, newline: bool) -> io::Result<()> {
        self.flush_text()?;
        let number = self.line_number;
        self.number += u64::from(newline);
        let through = self.number + u64::from(!newline);
        self.line_number = self.number + 1;
        let pending = std::mem::take(&mut self.pending).finish();
        self.pending_content = false;
        let mut raw = Spool::default();
        let mut literal = false;
        let mut tagged = false;
        for record in Records::new(pending.clone()) {
            let record = record?;
            if matches!(record.kind, TEXT | LITERAL) {
                literal |= record.kind == LITERAL;
                tagged |= record.a != 0;
                io::copy(&mut record.payload.reader(), &mut raw)?;
            } else {
                raw.write_all(b"\x1b")?;
            }
        }
        let (encoding, mut plain) = Encoding::of(&raw.finish())?;
        plain &= !literal;
        if plain {
            for record in Records::new(pending.clone()) {
                let record = record?;
                if record.kind == TEXT {
                    record.payload.utf8(|text| {
                        plain &= text.chars().all(|c| !c.is_control() || c == '\t');
                        Ok(())
                    })?;
                }
            }
        }
        // Streaming is valid only when the entire row has no horizontal edits.
        // A later cursor operation can still overwrite an earlier literal span.
        let mut linear = !plain && matches!(encoding, Encoding::Utf8);
        if linear {
            for record in Records::new(pending.clone()) {
                let record = record?;
                if matches!(record.kind, TEXT | LITERAL)
                    && !super::linear::check(&record.payload, record.kind == LITERAL)?
                {
                    linear = false;
                    break;
                }
            }
        }
        // An attributed capture can still contain rows with no source tags.
        // Only tagged rows need cell layout to carry attribution through edits.
        if tagged {
            plain = false;
            linear = false;
        }
        let decoding = encoding.facts();
        let mut display = Display::default();
        display.view = if self.matches.is_some() { 0 } else { self.line_view };
        self.line_view = self.view;
        let mut fragments = Fragments::default();
        for record in Records::new(pending) {
            let record = record?;
            match record.kind {
                TEXT | LITERAL => {
                    if plain {
                        record.payload.utf8(|text| fragments.push(display.view, Part::Text(text.to_string())))?;
                    } else if linear {
                        super::linear::render(&record.payload, record.kind == LITERAL, |text| {
                            fragments.push(display.view, Part::Text(text.to_string()))
                        })?;
                    } else if self.matches.is_some() {
                        let tags =
                            if record.a == 0 { &[][..] } else { self.tagged_records[record.a as usize - 1].as_slice() };
                        encoding.render_tagged(&record.payload, record.kind == LITERAL, tags, &mut display)?;
                    } else {
                        encoding.render(&record.payload, record.kind == LITERAL, &mut display)?;
                    }
                }
                IMAGE | UNREADABLE => {
                    display.take(|view, part| fragments.push(view, part))?;
                    let part = if record.kind == IMAGE {
                        Part::Image(Image {
                            mime: records::mime(record.c)?,
                            width: record.a as u32,
                            height: record.b as u32,
                            bytes: record.payload,
                        })
                    } else {
                        Part::Unreadable(record.payload.text()?)
                    };
                    fragments.push(display.view, part)?;
                }
                VIEW if self.matches.is_none() => display.view = record.a,
                VIEW => {}
                BOUNDARY => {}
                _ => return Err(io::Error::other("invalid terminal record")),
            }
        }
        display.take(|view, part| fragments.push(view, part))?;
        let ending_tag = self
            .matches
            .as_ref()
            .map_or(self.view, |ranges| super::write_notes::tag(ranges, self.position().saturating_sub(1)));
        fragments.finish(&mut self.queued, number, through, ending_tag, newline, decoding)
    }
}

#[derive(Serialize, Deserialize)]
struct Header {
    number: u64,
    through: u64,
    offset: usize,
    characters: usize,
    images: u64,
    graphics: u64,
    view: u64,
    newline: bool,
    decoding: Decoding,
}
struct Queue {
    bytes: Bytes,
    at: usize,
}
impl Queue {
    fn new(bytes: Bytes) -> Self {
        Self { bytes, at: 0 }
    }
    fn next(&mut self) -> io::Result<Option<Line<'static>>> {
        if self.at == self.bytes.len() {
            return Ok(None);
        }
        let length =
            u64::from_le_bytes(self.bytes.slice(self.at..self.at + 8).to_vec()?.try_into().expect("length")) as usize;
        self.at += 8;
        let header: Header =
            serde_json::from_slice(&self.bytes.slice(self.at..self.at + length).to_vec()?).map_err(io::Error::other)?;
        self.at += length;
        let length =
            u64::from_le_bytes(self.bytes.slice(self.at..self.at + 8).to_vec()?.try_into().expect("length")) as usize;
        self.at += 8;
        let parts = Parts::from_records(self.bytes.slice(self.at..self.at + length));
        self.at += length;
        Ok(Some(Line {
            number: header.number,
            through: header.through,
            offset: header.offset,
            characters: header.characters,
            images: header.images,
            graphics: header.graphics,
            view: header.view,
            newline: header.newline,
            decoding: header.decoding,
            parts,
        }))
    }
}
#[derive(Default)]
struct Fragments {
    parts: Spool,
    index: Spool,
    view: Option<u64>,
    start: u64,
    offset: usize,
    characters: usize,
    images: u64,
    graphics: u64,
}
impl Fragments {
    fn close(&mut self) -> io::Result<()> {
        if let Some(view) = self.view {
            for word in [view, self.start, self.parts.len(), self.offset as u64, self.characters as u64] {
                self.index.write_all(&word.to_le_bytes())?;
            }
            self.start = self.parts.len();
            self.offset = self.characters;
        }
        Ok(())
    }
    fn push(&mut self, view: u64, part: Part) -> io::Result<()> {
        if self.view != Some(view) {
            self.close()?;
            self.view = Some(view);
        }
        let size = match &part {
            Part::Text(text) => text.chars().count(),
            Part::Spaces(n) => *n,
            _ => 0,
        };
        self.images += u64::from(matches!(&part, Part::Image(_)));
        self.graphics += u64::from(matches!(&part, Part::Unreadable(_)));
        self.characters = self.characters.checked_add(size).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "terminal row exceeds the addressable character count")
        })?;
        Parts::write(&mut self.parts, &part)
    }
    fn finish(
        mut self,
        out: &mut Spool,
        number: u64,
        through: u64,
        view: u64,
        newline: bool,
        decoding: Decoding,
    ) -> io::Result<()> {
        if self.view != Some(view) {
            self.close()?;
            self.view = Some(view);
        }
        self.close()?;
        let parts = self.parts.finish();
        let index = self.index.finish();
        let mut input = index.reader();
        let count = index.len() / 40;
        for at in 0..count {
            let mut bytes = [0; 40];
            input.read_exact(&mut bytes)?;
            let words: Vec<_> = bytes.chunks_exact(8).map(|b| u64::from_le_bytes(b.try_into().unwrap())).collect();
            let header = Header {
                number,
                through,
                offset: words[3] as usize,
                characters: self.characters,
                images: self.images,
                graphics: self.graphics,
                view: words[0],
                newline: newline && at + 1 == count,
                decoding: decoding.clone(),
            };
            let header = serde_json::to_vec(&header).map_err(io::Error::other)?;
            out.write_all(&(header.len() as u64).to_le_bytes())?;
            out.write_all(&header)?;
            out.write_all(&(words[2] - words[1]).to_le_bytes())?;
            io::copy(&mut parts.slice(words[1] as usize..words[2] as usize).reader(), out)?;
        }
        Ok(())
    }
}

enum Encoding {
    Utf8,
    Legacy(CodePage),
    Escaped,
}
impl Encoding {
    /// Attribution is a separate replay. Its labels never enter model selection or decoding.
    fn render_tagged(
        &self,
        bytes: &Bytes,
        literal: bool,
        tags: &[(std::ops::Range<usize>, u64)],
        display: &mut Display,
    ) -> io::Result<()> {
        if matches!(self, Self::Utf8) {
            return display.write_tagged(bytes, literal, tags);
        }
        if matches!(self, Self::Escaped) {
            return Self::render_escaped_tagged(bytes, literal, tags, display);
        }
        // Keep decoder state across source boundaries; labels follow the bytes producing text.
        let mut decoded = Spool::default();
        let mut mapped = Vec::new();
        let mut offset = 0usize;
        let mut emit = |text: &str, source: usize| -> io::Result<()> {
            let tag = super::display::source_tag(tags, source);
            if tag != 0 && !text.is_empty() {
                mapped.push((decoded.len() as usize..decoded.len() as usize + text.len(), tag));
            }
            decoded.write_all(text.as_bytes())
        };
        match self {
            Self::Legacy(page) => {
                let mut decoder = LegacyDecoder::new(*page)?;
                for chunk in bytes.chunks() {
                    for byte in chunk? {
                        let text = decoder
                            .feed(&[byte], false)
                            .map_err(|_| io::Error::other("terminal source does not decode"))?;
                        emit(&text, offset)?;
                        offset += 1;
                    }
                }
                let text = decoder.feed(&[], true).map_err(|_| io::Error::other("terminal source does not decode"))?;
                emit(&text, offset.saturating_sub(1))?;
            }
            Self::Escaped => unreachable!(),
            Self::Utf8 => unreachable!(),
        }
        display.legacy = matches!(self, Self::Legacy(_));
        display.write_tagged(&decoded.finish(), literal, &mapped)
    }

    fn render_escaped_tagged(
        bytes: &Bytes,
        literal: bool,
        tags: &[(std::ops::Range<usize>, u64)],
        display: &mut Display,
    ) -> io::Result<()> {
        display.legacy = false;
        let mut pending = Vec::new();
        let mut valid = Spool::default();
        let mut mapped = Vec::new();
        let mut offset = 0usize;
        for chunk in bytes.chunks().chain(std::iter::once(Ok(Vec::new()))) {
            let chunk = chunk?;
            let end = chunk.is_empty();
            pending.extend_from_slice(&chunk);
            let mut at = 0;
            while at < pending.len() {
                let (good, bad) = match std::str::from_utf8(&pending[at..]) {
                    Ok(text) => (text.len(), 0),
                    Err(error) => (
                        error.valid_up_to(),
                        error.error_len().unwrap_or(if end { pending.len() - at - error.valid_up_to() } else { 0 }),
                    ),
                };
                for character in std::str::from_utf8(&pending[at..at + good]).expect("valid prefix").chars() {
                    let tag = super::display::source_tag(tags, offset + at);
                    let start = valid.len() as usize;
                    valid.write_all(character.encode_utf8(&mut [0; 4]).as_bytes())?;
                    if tag != 0 {
                        mapped.push((start..start + character.len_utf8(), tag));
                    }
                    at += character.len_utf8();
                }
                if bad > 0 {
                    display.write_tagged(&std::mem::take(&mut valid).finish(), literal, &mapped)?;
                    mapped.clear();
                    for byte in &pending[at..at + bad] {
                        display.view = super::display::source_tag(tags, offset + at);
                        // The canonical renderer gives an escaped byte one cell, not four.
                        display.byte(*byte)?;
                        at += 1;
                    }
                }
                if bad == 0 {
                    break;
                }
            }
            pending.drain(..at);
            offset += at;
        }
        display.write_tagged(&valid.finish(), literal, &mapped)
    }

    fn of(bytes: &Bytes) -> io::Result<(Self, bool)> {
        if bytes.as_slice().is_some_and(|bytes| std::str::from_utf8(bytes).is_ok()) {
            return Ok((Self::Utf8, true));
        }
        let mut utf8 = TextDecoder::new(TextEncoding::utf8());
        let mut valid = true;
        for chunk in bytes.chunks() {
            valid &= utf8.feed(&chunk?, false).is_ok();
        }
        valid &= utf8.feed(&[], true).is_ok();
        if valid {
            return Ok((Self::Utf8, true));
        }
        if bytes.len() <= i32::MAX as usize {
            for page in code_pages() {
                let mut decoder = LegacyDecoder::new(page)?;
                let mut valid = true;
                for chunk in bytes.chunks() {
                    valid &= decoder.feed(&chunk?, false).is_ok();
                }
                valid &= decoder.feed(&[], true).is_ok();
                if valid {
                    return Ok((Self::Legacy(page), false));
                }
            }
        }
        Ok((Self::Escaped, false))
    }
    fn facts(&self) -> Decoding {
        match self {
            Self::Utf8 => Decoding::default(),
            Self::Legacy(page) => Decoding { legacy: vec![page.name()], escaped: false },
            Self::Escaped => Decoding { legacy: Vec::new(), escaped: true },
        }
    }
    fn render(&self, bytes: &Bytes, literal: bool, display: &mut Display) -> io::Result<()> {
        display.legacy = matches!(self, Self::Legacy(_));
        match self {
            Self::Utf8 => display.write(bytes, literal),
            Self::Legacy(page) => {
                let mut decoder = LegacyDecoder::new(*page)?;
                let mut text = Spool::default();
                for chunk in bytes.chunks() {
                    let chunk = decoder.feed(&chunk?, false).map_err(|_| {
                        io::Error::new(
                            io::ErrorKind::InvalidData,
                            "terminal fragment does not decode in its line's code page",
                        )
                    })?;
                    text.write_all(chunk.as_bytes())?;
                }
                text.write_all(
                    decoder
                        .feed(&[], true)
                        .map_err(|_| {
                            io::Error::new(
                                io::ErrorKind::InvalidData,
                                "terminal fragment does not decode in its line's code page",
                            )
                        })?
                        .as_bytes(),
                )?;
                display.write(&text.finish(), literal)
            }
            Self::Escaped => {
                let mut pending = Vec::new();
                let mut valid = Spool::default();
                for chunk in bytes.chunks().chain(std::iter::once(Ok(Vec::new()))) {
                    let chunk = chunk?;
                    let end = chunk.is_empty();
                    pending.extend_from_slice(&chunk);
                    let mut at = 0;
                    while at < pending.len() {
                        let (good, bad) = match std::str::from_utf8(&pending[at..]) {
                            Ok(text) => (text.len(), 0),
                            Err(e) => (
                                e.valid_up_to(),
                                e.error_len().unwrap_or(if end { pending.len() - at - e.valid_up_to() } else { 0 }),
                            ),
                        };
                        valid.write_all(&pending[at..at + good])?;
                        at += good;
                        if bad > 0 {
                            display.write(&std::mem::take(&mut valid).finish(), literal)?;
                            for byte in &pending[at..at + bad] {
                                display.byte(*byte)?;
                            }
                        }
                        at += bad;
                        if bad == 0 {
                            break;
                        }
                    }
                    pending.drain(..at);
                }
                display.write(&valid.finish(), literal)
            }
        }
    }
}
