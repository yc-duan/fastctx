//! Reading a job's raw output the way a terminal would show it.
//!
//! The log keeps every byte. What a resident is shown is what a person at a
//! terminal would see: a carriage return moves back to the start of the
//! line and what follows overwrites it, so a progress bar shows its last
//! state; colours and other control sequences are not shown; images drawn
//! with the iTerm2 (OSC 1337) or kitty graphics protocols become images.
//! Text is decoded line by line: UTF-8, or on Windows the system's code
//! page when a line is not UTF-8, and any byte that decodes in neither is
//! shown as `\xNN`, never dropped or replaced.

use std::io::{self, Read};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use fastctx_platform::decode_legacy;

use crate::image::sniff;

/// One raw line of output (ended by `\n`, or by the end of the output), as
/// shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    /// Counting from 1, as `fastctx view --lines` counts the log's lines.
    pub number: u64,
    pub parts: Vec<Part>,
    pub decoding: Decoding,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Part {
    Text(String),
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decoding {
    Utf8,
    /// Decoded in the system's legacy code page, named here.
    Legacy(String),
    /// Not decodable; the bytes that would not decode are shown as `\xNN`.
    Escaped,
}

/// Reads output and yields its lines one at a time.
pub struct Lines<R> {
    input: R,
    buffer: Vec<u8>,
    start: usize,
    end: usize,
    number: u64,
    state: State,
    /// The current line's bytes and the images found in it so far, in order.
    pending: Vec<Pending>,
    text: Vec<u8>,
    /// A kitty image sent in several chunks, gathered so far.
    kitty: Option<Vec<u8>>,
    finished: bool,
}

enum Pending {
    Text(Vec<u8>),
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
            state: State::Normal,
            pending: Vec::new(),
            text: Vec::new(),
            kitty: None,
            finished: false,
        }
    }

    /// The next line, or `None` after the last.
    pub fn next_line(&mut self) -> io::Result<Option<Line>> {
        let mut sequence = Vec::new();
        loop {
            if self.start == self.end {
                if self.finished {
                    return Ok(None);
                }
                let read = self.input.read(&mut self.buffer)?;
                if read == 0 {
                    self.finished = true;
                    return Ok(self.finish(&sequence));
                }
                self.start = 0;
                self.end = read;
            }
            let byte = self.buffer[self.start];
            self.start += 1;
            match self.state {
                State::Normal => match byte {
                    b'\n' => return Ok(Some(self.end_line())),
                    0x1b => self.state = State::Escape,
                    _ => self.text.push(byte),
                },
                State::Escape => match byte {
                    b']' => self.state = State::String { apc: false, escape: false },
                    b'_' => self.state = State::String { apc: true, escape: false },
                    // Any other sequence stays in the text, where the line
                    // renderer reads it as a terminal would.
                    _ => {
                        self.text.extend_from_slice(&[0x1b, byte]);
                        self.state = State::Normal;
                        if byte == b'\n' {
                            self.text.pop();
                            return Ok(Some(self.end_line()));
                        }
                    }
                },
                State::String { apc, escape } => match byte {
                    0x07 if !apc => {
                        self.close_string(apc, &sequence);
                        sequence.clear();
                        self.state = State::Normal;
                    }
                    b'\\' if escape => {
                        self.close_string(apc, &sequence);
                        sequence.clear();
                        self.state = State::Normal;
                    }
                    0x1b => self.state = State::String { apc, escape: true },
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

    fn finish(&mut self, sequence: &[u8]) -> Option<Line> {
        if let State::String { apc, .. } = self.state {
            let what = if apc { "kitty" } else { "OSC" };
            if is_image_sequence(apc, sequence) {
                self.push_unreadable(format!(
                    "an image ({what} sequence) cut off before its end, {} bytes",
                    sequence.len()
                ));
            }
        }
        if self.kitty.take().is_some() {
            self.push_unreadable("a kitty image whose last chunk never came".to_string());
        }
        if self.text.is_empty() && self.pending.is_empty() {
            return None;
        }
        Some(self.end_line())
    }

    fn close_string(&mut self, apc: bool, body: &[u8]) {
        if apc {
            if let Some(control_and_payload) = body.strip_prefix(b"G") {
                self.kitty_chunk(control_and_payload);
            }
        } else if let Some(rest) = body.strip_prefix(b"1337;File=") {
            self.iterm_file(rest);
        }
        // Other strings (window titles, hyperlinks, kitty commands that are
        // not images) are not shown, as a terminal does not show them.
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

    fn end_line(&mut self) -> Line {
        self.flush_text();
        self.number += 1;
        // The line's text is decoded as one piece, so a multi-byte character
        // split around an image still decodes.
        let mut parts = Vec::new();
        let mut decoding = Decoding::Utf8;
        for pending in std::mem::take(&mut self.pending) {
            match pending {
                Pending::Text(bytes) => {
                    let (text, how) = render(&bytes);
                    decoding = worse(decoding, how);
                    parts.push(Part::Text(text));
                }
                Pending::Image(image) => parts.push(Part::Image(image)),
                Pending::Unreadable(why) => parts.push(Part::Unreadable(why)),
            }
        }
        Line { number: self.number, parts, decoding }
    }
}

fn is_image_sequence(apc: bool, body: &[u8]) -> bool {
    if apc { body.starts_with(b"G") } else { body.starts_with(b"1337;File=") }
}

fn worse(a: Decoding, b: Decoding) -> Decoding {
    match (a, b) {
        (Decoding::Escaped, _) | (_, Decoding::Escaped) => Decoding::Escaped,
        (Decoding::Legacy(name), _) | (_, Decoding::Legacy(name)) => Decoding::Legacy(name),
        (Decoding::Utf8, Decoding::Utf8) => Decoding::Utf8,
    }
}

fn trim_ascii_whitespace(bytes: &[u8]) -> &[u8] {
    bytes.trim_ascii()
}

/// Decodes one line's bytes and plays them as a terminal would.
fn render(bytes: &[u8]) -> (String, Decoding) {
    let bytes = bytes.strip_suffix(b"\r").unwrap_or(bytes);
    let (text, decoding) = decode(bytes);
    (play(&text), decoding)
}

fn decode(bytes: &[u8]) -> (String, Decoding) {
    if let Ok(text) = std::str::from_utf8(bytes) {
        return (text.to_string(), Decoding::Utf8);
    }
    if let Some((text, name)) = decode_legacy(bytes) {
        return (text, Decoding::Legacy(name));
    }
    let mut text = String::new();
    for chunk in bytes.utf8_chunks() {
        text.push_str(chunk.valid());
        for byte in chunk.invalid() {
            text.push_str(&format!("\\x{byte:02X}"));
        }
    }
    (text, Decoding::Escaped)
}

/// Carriage return, backspace, erase-in-line and the dropping of every
/// other control sequence, on one line.
fn play(text: &str) -> String {
    let mut cells: Vec<char> = Vec::new();
    let mut cursor = 0usize;
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match c {
            '\r' => cursor = 0,
            '\u{8}' => cursor = cursor.saturating_sub(1),
            '\t' => put(&mut cells, &mut cursor, '\t'),
            // Any other escape sequence is two characters with nothing to show.
            '\u{1b}' => {
                if chars.next() == Some('[') {
                    let mut parameters = String::new();
                    let mut last = None;
                    for c in chars.by_ref() {
                        if ('\u{40}'..='\u{7e}').contains(&c) {
                            last = Some(c);
                            break;
                        }
                        parameters.push(c);
                    }
                    if last == Some('K') {
                        match parameters.as_str() {
                            "" | "0" => cells.truncate(cursor),
                            "1" => cells.iter_mut().take(cursor + 1).for_each(|cell| *cell = ' '),
                            "2" => cells.iter_mut().for_each(|cell| *cell = ' '),
                            _ => {}
                        }
                    }
                }
            }
            c if c.is_control() => {}
            c => put(&mut cells, &mut cursor, c),
        }
    }
    cells.into_iter().collect()
}

fn put(cells: &mut Vec<char>, cursor: &mut usize, c: char) {
    if *cursor < cells.len() {
        cells[*cursor] = c;
    } else {
        cells.resize(*cursor, ' ');
        cells.push(c);
    }
    *cursor += 1;
}
