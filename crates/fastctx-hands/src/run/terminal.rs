//! Reading a job's raw output the way a terminal would show it.
//!
//! The log keeps every byte. What a resident is shown is what a person at a
//! terminal would see: a carriage return moves back to the start of the
//! line and what follows overwrites it, so a progress bar shows its last
//! state; recognized presentation controls are applied; images drawn
//! with the iTerm2 (OSC 1337) or kitty graphics protocols become images.
//! Text is decoded line by line: UTF-8, or on Windows the system's code
//! page when a line is not UTF-8, and any byte that decodes in neither is
//! shown as `\xNN`, never dropped or replaced. Unknown and incomplete
//! instructions remain visible rather than consuming later text.

use std::collections::{BTreeSet, VecDeque};
use std::io::{self, Read};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use fastctx_platform::legacy_text::{CodePage, decode_with_code_page};

use super::display::Display;
use crate::image::sniff;

/// A contiguous budget region of a rendered row, with its raw log location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    /// Counting from 1, as `fastctx view --lines` counts the log's lines.
    pub number: u64,
    /// Last raw line contributing to this displayed row (strings may span lines).
    pub through: u64,
    pub offset: usize,
    pub characters: usize,
    pub parts: Vec<Part>,
    pub decoding: Decoding,
    /// A view region's identity, or zero for ordinary output.
    pub view: u64,
    /// Whether the fragment ends at a physical newline in the log.
    pub newline: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Part {
    Text(String),
    /// Cursor-created blank columns, kept sparse until a window is chosen.
    Spaces(usize),
    Image(Image),
    /// A graphics sequence that cannot be shown as an image, and why.
    Unreadable(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub mime: &'static str,
    pub width: u32,
    pub height: u32,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Decoding {
    pub legacy: Vec<String>,
    pub escaped: bool,
}

/// Reads output and yields its lines one at a time.
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
    /// The current line's bytes and the images found in it so far, in order.
    pending: Vec<Pending>,
    text: Vec<u8>,
    /// A kitty image sent in several chunks, gathered so far.
    kitty: Option<Vec<u8>>,
    finished: bool,
    view: u64,
    line_view: u64,
    next_view: u64,
    closed_views: BTreeSet<u64>,
    ready: VecDeque<Line>,
}

/// What `fastctx view` writes first inside a World `run`: an OSC string no
/// terminal shows, by which the answer knows the output is files the
/// resident asked to read rather than a program's output.
pub const VIEW_MARK: &str = "\x1b]7701;fastctx;view\x07";
pub const VIEW_END: &str = "\x1b]7701;fastctx;end\x07";

enum Pending {
    Text(Vec<u8>),
    Literal(Vec<u8>),
    Boundary,
    View(u64),
    Image(Image),
    Unreadable(String),
}

#[derive(Clone, Copy)]
enum State {
    Normal,
    /// After ESC, deciding what kind of sequence follows.
    Escape,
    /// Inside an OSC (`ESC ]`) or APC (`ESC _`) string, until BEL or `ESC \`.
    String {
        apc: bool,
        escape: bool,
    },
}

impl<R: Read> Lines<R> {
    pub fn new(input: R) -> Self {
        Self {
            input,
            buffer: vec![0; 64 * 1024],
            start: 0,
            end: 0,
            number: 0,
            line_number: 1,
            raw_newlines: 0,
            raw_unterminated: false,
            state: State::Normal,
            pending: Vec::new(),
            text: Vec::new(),
            kitty: None,
            finished: false,
            view: 0,
            line_view: 0,
            next_view: 1,
            closed_views: BTreeSet::new(),
            ready: VecDeque::new(),
        }
    }

    /// The next line, or `None` after the last.
    pub fn next_line(&mut self) -> io::Result<Option<Line>> {
        let mut sequence = Vec::new();
        loop {
            if let Some(line) = self.ready.pop_front() {
                return Ok(Some(line));
            }
            if self.start == self.end {
                if self.finished {
                    return Ok(None);
                }
                let read = self.input.read(&mut self.buffer)?;
                if read == 0 {
                    self.finished = true;
                    self.finish(&sequence)?;
                    return Ok(self.ready.pop_front());
                }
                self.start = 0;
                self.end = read;
            }
            let byte = self.buffer[self.start];
            self.start += 1;
            self.raw_unterminated = byte != b'\n';
            self.raw_newlines += u64::from(byte == b'\n');
            match self.state {
                State::Normal => match byte {
                    b'\n' => return self.end_line(true).map(Some),
                    0x1b => self.state = State::Escape,
                    _ => self.text.push(byte),
                },
                State::Escape => match byte {
                    b']' => self.state = State::String { apc: false, escape: false },
                    b'_' => self.state = State::String { apc: true, escape: false },
                    0x1b => self.text.push(0x1b),
                    // Any other sequence stays in the text, where the line
                    // renderer reads it as a terminal would.
                    _ => {
                        self.text.extend_from_slice(&[0x1b, byte]);
                        self.state = State::Normal;
                        if byte == b'\n' {
                            self.text.pop();
                            return self.end_line(true).map(Some);
                        }
                    }
                },
                State::String { apc, escape } => match byte {
                    byte if escape && byte != b'\\' => {
                        self.literal_string(apc, &sequence)?;
                        sequence.clear();
                        // ESC starts a new instruction unless it forms ST.
                        // Keep the interrupted string visible, then parse the
                        // new instruction without losing its first byte.
                        self.start -= 1;
                        self.raw_newlines -= u64::from(byte == b'\n');
                        self.state = State::Escape;
                    }
                    0x18 | 0x1a => {
                        self.literal_string(apc, &sequence)?;
                        sequence.clear();
                        self.text.push(byte);
                        self.state = State::Normal;
                    }
                    0x07 if !apc => {
                        self.close_string(apc, &sequence)?;
                        sequence.clear();
                        self.state = State::Normal;
                    }
                    b'\\' if escape => {
                        self.close_string(apc, &sequence)?;
                        sequence.clear();
                        self.state = State::Normal;
                    }
                    0x1b => {
                        if escape {
                            sequence.push(0x1b);
                        }
                        self.state = State::String { apc, escape: true };
                    }
                    _ => {
                        if escape {
                            sequence.push(0x1b);
                        }
                        sequence.push(byte);
                        self.state = State::String { apc, escape: false };
                    }
                },
            }
        }
    }

    pub fn raw_lines(&self) -> u64 {
        self.raw_newlines + u64::from(self.raw_unterminated)
    }

    fn finish(&mut self, sequence: &[u8]) -> io::Result<()> {
        if let State::String { apc, escape } = self.state {
            let what = if apc { "kitty" } else { "OSC" };
            if is_image_sequence(apc, sequence) {
                self.push_unreadable(format!(
                    "an image ({what} sequence) cut off before its end, {} bytes",
                    sequence.len()
                ));
            }
            let mut sequence = sequence.to_vec();
            if escape {
                sequence.push(0x1b);
            }
            self.literal_string(apc, &sequence)?;
        } else if matches!(self.state, State::Escape) {
            self.text.push(0x1b);
        }
        if self.kitty.take().is_some() {
            self.push_unreadable("a kitty image whose last chunk never came".to_string());
        }
        if self.text.is_empty() && self.pending.iter().all(|part| matches!(part, Pending::View(_) | Pending::Boundary))
        {
            return Ok(());
        }
        let at = self.ready.len();
        let line = self.end_line(false)?;
        self.ready.insert(at, line);
        Ok(())
    }

    fn literal_string(&mut self, apc: bool, sequence: &[u8]) -> io::Result<()> {
        let mut literal = vec![0x1b, if apc { b'_' } else { b']' }];
        literal.extend_from_slice(sequence);
        // Replaying an unfinished string through the parser could swallow
        // it again. Only physical newlines retain their layout meaning.
        self.flush_text();
        for (i, bytes) in literal.split(|&byte| byte == b'\n').enumerate() {
            if i > 0 {
                let at = self.ready.len();
                let line = self.end_line(true)?;
                self.ready.insert(at, line);
            }
            if !bytes.is_empty() {
                self.pending.push(Pending::Literal(bytes.to_vec()));
            }
        }
        Ok(())
    }

    fn close_string(&mut self, apc: bool, body: &[u8]) -> io::Result<()> {
        // Even an invisible title or hyperlink separates encoded characters;
        // removing it must not join two invalid byte fragments into UTF-8.
        self.flush_text();
        self.pending.push(Pending::Boundary);
        if apc {
            if let Some(control_and_payload) = body.strip_prefix(b"G") {
                self.kitty_chunk(control_and_payload);
            }
        } else if let Some(rest) = body.strip_prefix(b"1337;File=") {
            self.iterm_file(rest);
        } else if VIEW_MARK.as_bytes()[2..VIEW_MARK.len() - 1] == *body {
            self.change_view(true)?;
        } else if VIEW_END.as_bytes()[2..VIEW_END.len() - 1] == *body {
            self.change_view(false)?;
        }
        // Other strings (window titles, hyperlinks, kitty commands that are
        // not images) are not shown, as a terminal does not show them.
        self.number += body.iter().filter(|&&byte| byte == b'\n').count() as u64;
        Ok(())
    }

    /// Only a closed view region can grant the larger budget. If a pipe
    /// cuts off its end marker, later program output must not inherit it.
    pub fn closed_views(&self) -> &BTreeSet<u64> {
        &self.closed_views
    }

    fn change_view(&mut self, begin: bool) -> io::Result<()> {
        let view = if begin {
            let id = self.next_view;
            self.next_view += 1;
            id
        } else {
            if self.view != 0 {
                self.closed_views.insert(self.view);
            }
            0
        };
        self.flush_text();
        self.pending.push(Pending::View(view));
        self.view = view;
        Ok(())
    }

    fn iterm_file(&mut self, rest: &[u8]) {
        let Some(colon) = rest.iter().position(|&b| b == b':') else {
            return self.push_unreadable("an iTerm2 image without data".to_string());
        };
        let arguments = &rest[..colon];
        let inline = arguments.split(|&b| b == b';').any(|argument| argument == b"inline=1");
        if !inline {
            // Without inline=1 a terminal offers the file as a download.
            return;
        }
        match STANDARD.decode(trim_ascii_whitespace(&rest[colon + 1..])) {
            Ok(bytes) => self.push_image(bytes),
            Err(error) => self.push_unreadable(format!("an iTerm2 image whose base64 does not decode: {error}")),
        }
    }

    fn kitty_chunk(&mut self, control_and_payload: &[u8]) {
        let (control, payload) = match control_and_payload.iter().position(|&b| b == b';') {
            Some(split) => (&control_and_payload[..split], &control_and_payload[split + 1..]),
            None => (control_and_payload, &[][..]),
        };
        let mut format = None;
        let mut medium = b"d".as_slice();
        let mut more = false;
        let mut action = b"t".as_slice();
        for pair in control.split(|&b| b == b',') {
            match pair.split_first() {
                Some((b'f', value)) => format = value.strip_prefix(b"="),
                Some((b't', value)) => medium = value.strip_prefix(b"=").unwrap_or(medium),
                Some((b'm', value)) => more = value == b"=1",
                Some((b'a', value)) => action = value.strip_prefix(b"=").unwrap_or(action),
                _ => {}
            }
        }
        let continuing = self.kitty.is_some();
        if !continuing && !matches!(action, b"t" | b"T") {
            // Commands that place or delete images sent earlier draw nothing new.
            return;
        }
        if !continuing && (format != Some(b"100") || medium != b"d") {
            return self.push_unreadable(
                "a kitty image that is not a PNG sent directly (only f=100, t=d is shown)".to_string(),
            );
        }
        let gathered = self.kitty.get_or_insert_with(Vec::new);
        gathered.extend_from_slice(trim_ascii_whitespace(payload));
        if more {
            return;
        }
        let gathered = self.kitty.take().expect("set above");
        match STANDARD.decode(&gathered) {
            Ok(bytes) => self.push_image(bytes),
            Err(error) => self.push_unreadable(format!("a kitty image whose base64 does not decode: {error}")),
        }
    }

    fn push_image(&mut self, bytes: Vec<u8>) {
        match sniff(&bytes) {
            Some((mime, width, height)) => {
                self.flush_text();
                self.pending.push(Pending::Image(Image { mime, width, height, bytes }));
            }
            None => self.push_unreadable(format!(
                "an image of {} bytes in a format other than PNG, JPEG, GIF or WebP",
                bytes.len()
            )),
        }
    }

    fn push_unreadable(&mut self, why: String) {
        self.flush_text();
        self.pending.push(Pending::Unreadable(why));
    }

    fn flush_text(&mut self) {
        if !self.text.is_empty() {
            self.pending.push(Pending::Text(std::mem::take(&mut self.text)));
        }
    }

    fn end_line(&mut self, newline: bool) -> io::Result<Line> {
        self.flush_text();
        let number = self.line_number;
        if newline {
            self.number += 1;
        }
        let through = self.number + u64::from(!newline);
        self.line_number = self.number + 1;
        let mut groups: Vec<(u64, Vec<Part>)> = Vec::new();
        let raw: Vec<u8> = self
            .pending
            .iter()
            .flat_map(|pending| match pending {
                Pending::Text(bytes) | Pending::Literal(bytes) => bytes.as_slice(),
                // Protocol boundaries contain ASCII controls, which cannot be
                // part of a UTF-8 or Windows multibyte character.
                _ => b"\x1b".as_slice(),
            })
            .copied()
            .collect();
        let encoding = Encoding::of(&raw);
        drop(raw);
        let decoding = encoding.facts();
        // Plain output needs no per-cell layout. Keep long ordinary lines
        // proportional to their byte size rather than allocating a tree node
        // for every character.
        let plain = matches!(encoding, Encoding::Utf8)
            && self.pending.iter().all(|pending| match pending {
                Pending::Text(bytes) => {
                    std::str::from_utf8(bytes).is_ok_and(|text| text.chars().all(|c| !c.is_control() || c == '\t'))
                }
                Pending::Literal(_) => false,
                _ => true,
            });
        let mut display = Display::default();
        display.view = self.line_view;
        self.line_view = self.view;
        for pending in std::mem::take(&mut self.pending) {
            let literal = matches!(pending, Pending::Literal(_));
            match pending {
                Pending::Text(bytes) | Pending::Literal(bytes) => {
                    if plain {
                        groups.push((
                            display.view,
                            vec![Part::Text(String::from_utf8(bytes).map_err(io::Error::other)?)],
                        ));
                    } else {
                        encoding.render(&bytes, literal, &mut display)?;
                    }
                }
                Pending::Image(image) => {
                    groups.extend(display.take()?);
                    groups.push((display.view, vec![Part::Image(image)]));
                }
                Pending::Unreadable(why) => {
                    groups.extend(display.take()?);
                    groups.push((display.view, vec![Part::Unreadable(why)]));
                }
                Pending::View(view) => display.view = view,
                Pending::Boundary => {}
            }
        }
        groups.extend(display.take()?);
        // OutputPiece character offsets use u64 and slices use usize. A
        // cursor may be sparse, but its reported extent must still fit both.
        let extent = groups
            .iter()
            .flat_map(|(_, parts)| parts)
            .try_fold(0usize, |sum, part| {
                let size = match part {
                    Part::Text(text) => text.chars().count(),
                    Part::Spaces(count) => *count,
                    _ => 0,
                };
                sum.checked_add(size)
            })
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "terminal row exceeds the addressable character count")
            })?;
        u64::try_from(extent).map_err(io::Error::other)?;
        let mut fragments: Vec<Line> = Vec::new();
        for (view, parts) in groups {
            if let Some(line) = fragments.last_mut().filter(|line| line.view == view) {
                line.parts.extend(parts);
            } else {
                fragments.push(Line {
                    number,
                    through,
                    offset: 0,
                    characters: extent,
                    parts,
                    decoding: decoding.clone(),
                    view,
                    newline: false,
                });
            }
        }
        if fragments.last().is_none_or(|line| line.view != self.view) {
            fragments.push(Line {
                number,
                through,
                offset: 0,
                characters: extent,
                parts: Vec::new(),
                decoding,
                view: self.view,
                newline: false,
            });
        }
        let mut offset = 0;
        for line in &mut fragments {
            line.offset = offset;
            offset += line
                .parts
                .iter()
                .map(|part| match part {
                    Part::Text(text) => text.chars().count(),
                    Part::Spaces(count) => *count,
                    _ => 0,
                })
                .sum::<usize>();
        }
        fragments.last_mut().expect("at least one fragment").newline = newline;
        let first = fragments.remove(0);
        self.ready.extend(fragments);
        Ok(first)
    }
}

fn is_image_sequence(apc: bool, body: &[u8]) -> bool {
    if apc { body.starts_with(b"G") } else { body.starts_with(b"1337;File=") }
}

fn trim_ascii_whitespace(bytes: &[u8]) -> &[u8] {
    bytes.trim_ascii()
}

enum Encoding {
    Utf8,
    Legacy(CodePage),
    Escaped,
}

impl Encoding {
    fn of(bytes: &[u8]) -> Self {
        if std::str::from_utf8(bytes).is_ok() {
            return Self::Utf8;
        }
        if let Some((_, encoding)) = decode_with_code_page(bytes) {
            return Self::Legacy(encoding);
        }
        Self::Escaped
    }

    fn facts(&self) -> Decoding {
        match self {
            Self::Utf8 => Decoding::default(),
            Self::Legacy(encoding) => Decoding { legacy: vec![encoding.name()], escaped: false },
            Self::Escaped => Decoding { legacy: Vec::new(), escaped: true },
        }
    }

    fn render(&self, bytes: &[u8], literal: bool, display: &mut Display) -> io::Result<()> {
        display.legacy = matches!(self, Self::Legacy(_));
        match self {
            Self::Utf8 => display.write(std::str::from_utf8(bytes).map_err(io::Error::other)?, literal),
            Self::Legacy(encoding) => {
                let text = encoding.decode(bytes).ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        "terminal fragment does not decode in its line's code page",
                    )
                })?;
                display.write(&text, literal);
            }
            Self::Escaped => {
                for chunk in bytes.utf8_chunks() {
                    display.write(chunk.valid(), literal);
                    for byte in chunk.invalid() {
                        display.byte(*byte);
                    }
                }
            }
        }
        Ok(())
    }
}
