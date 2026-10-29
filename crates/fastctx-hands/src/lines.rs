//! A disposable on-disk index of original ranges and decoding checkpoints.

mod projection;

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use crate::encoding::{self, TextDecoder, TextEncoding};
use crate::spool::{BLOCK, Bytes, Spool};
use fastctx_types::lines::{Direction, Match, Position};

const RECORD: u64 = 40;

/// The canonical output decoder, including its ordered native fallbacks.
pub fn output_policy() -> String {
    let pages: Vec<_> = fastctx_platform::legacy_text::code_pages().into_iter().map(|page| page.name()).collect();
    format!("terminal-v1:{pages:?}")
}

/// Why an output's index did not grow (`Index::grow`).
#[derive(Debug)]
pub enum NotGrown {
    /// The index is as it was before: the same growth can be tried again.
    Passing(String),
    /// What the index shows can no longer be trusted: the output was
    /// rewritten or truncated, or the index could not be put back.
    Ended(String),
}

/// The rows' file under their buffer. Told to drop, it takes what the
/// buffer hands it without writing it, so a reparse that failed leaves
/// none of its rows behind.
struct RowsFile {
    file: File,
    dropping: bool,
}

impl Write for RowsFile {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.dropping { Ok(bytes.len()) } else { self.file.write(bytes) }
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.dropping { Ok(()) } else { self.file.flush() }
    }
}

impl Seek for RowsFile {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        self.file.seek(to)
    }
}

struct Row {
    start: u64,
    end: u64,
    ordinal: u64,
    codec: u32,
    state: [u8; 3],
    terminated: bool,
    literal: bool,
}

pub struct Index {
    path: PathBuf,
    file: std::io::BufReader<File>,
    file_cursor: Option<u64>,
    rows: std::io::BufWriter<RowsFile>,
    row_cursor: u64,
    row_cache: Vec<u8>,
    row_cache_start: Option<u64>,
    codecs: Vec<TextEncoding>,
    newline: Vec<u8>,
    crlf: Vec<u8>,
    pub length: u64,
    pub bytes: u64,
    output: bool,
}

impl Index {
    /// The index is an anonymous file: exactly 40 bytes per logical line.
    /// Only requested lines are collected; decoding and search spool long rows.
    pub fn open(
        path: &Path,
        directory: &Path,
        output: bool,
        selection: Option<&str>,
        cancelled: Arc<AtomicBool>,
    ) -> Result<Self, String> {
        let file = fastctx_platform::file_read::open(path).map_err(err)?;
        let metadata = file.metadata().map_err(err)?;
        if !metadata.is_file() {
            return Err("the text source must be an ordinary file".into());
        }
        let mut index = Self {
            path: path.to_path_buf(),
            file: std::io::BufReader::with_capacity(BLOCK, file),
            file_cursor: None,
            rows: std::io::BufWriter::with_capacity(
                BLOCK,
                RowsFile { file: tempfile::tempfile_in(directory).map_err(err)?, dropping: false },
            ),
            row_cursor: 0,
            row_cache: Vec::new(),
            row_cache_start: None,
            codecs: Vec::new(),
            newline: b"\n".to_vec(),
            crlf: b"\r\n".to_vec(),
            length: 0,
            bytes: metadata.len(),
            output,
        };
        if output {
            index.append(0, false, cancelled)?;
        } else {
            index.inspect(selection, cancelled)?;
        }
        Ok(index)
    }

    fn inspect(&mut self, selection: Option<&str>, cancelled: Arc<AtomicBool>) -> Result<(), String> {
        let mut inspect = Input::new(self.file.get_ref().try_clone().map_err(err)?, self.bytes, cancelled.clone());
        let plan = encoding::file::Plan::inspect(&mut inspect, selection)?;
        (self.newline, self.crlf) = plan.line_endings();
        self.file.seek(SeekFrom::Start(0)).map_err(err)?;
        let mut state = encoding::iso2022::State::default();
        encoding::file::visit(
            Input::new(self.file.get_ref().try_clone().map_err(err)?, self.bytes, cancelled.clone()),
            &plan,
            |line| {
                check(&cancelled)?;
                let codec = match line.codec {
                    None => 0,
                    Some(codec) => {
                        let at = self.codecs.iter().position(|known| *known == codec).unwrap_or_else(|| {
                            self.codecs.push(codec);
                            self.codecs.len() - 1
                        });
                        u32::try_from(at + 1).map_err(err)?
                    }
                };
                let start = line.bytes.start as u64;
                let end = line.bytes.end as u64;
                let before = state.escape().try_into().expect("ISO-2022 switch");
                if line.codec.is_some_and(TextEncoding::is_stateful) {
                    // Reopen rather than seek a clone sharing the scan's offset.
                    let mut raw = fastctx_platform::file_read::open(&self.path).map_err(err)?;
                    raw.seek(SeekFrom::Start(start)).map_err(err)?;
                    let mut remaining = end - start;
                    let mut buffer = [0; BLOCK];
                    let mut tail = Vec::new();
                    while remaining > 0 {
                        let n = raw.read(&mut buffer[..remaining.min(BLOCK as u64) as usize]).map_err(err)?;
                        if n == 0 {
                            return Err("text source ended during indexing".into());
                        }
                        tail.extend_from_slice(&buffer[..n]);
                        state = state.after(&tail);
                        let retain = tail.len().min(2);
                        tail.drain(..tail.len() - retain);
                        remaining -= n as u64;
                    }
                }
                let terminated = line.text.ends_with(b"\n").map_err(err)?;
                self.push(Row { start, end, ordinal: 0, codec, state: before, terminated, literal: false })
            },
        )
    }

    fn append(&mut self, start: u64, literal: bool, cancelled: Arc<AtomicBool>) -> Result<(), String> {
        let mut file = fastctx_platform::file_read::open(&self.path).map_err(err)?;
        file.seek(SeekFrom::Start(start)).map_err(err)?;
        let prefix = if literal { crate::run::terminal::VIEW_MARK.as_bytes() } else { &[] };
        let mut lines = crate::run::terminal::Lines::new(std::io::Cursor::new(prefix).chain(Input::new(
            file,
            self.bytes - start,
            cancelled.clone(),
        )));
        let (mut group_start, mut group_end, mut ordinal) = (start, start, 0);
        let (mut entering_literal, mut group_literal) = (literal, literal);
        let mut number = None;
        let mut row: Option<Row> = None;
        loop {
            check(&cancelled)?;
            let Some(line) = lines.next_line().map_err(err)? else { break };
            let (terminated, current_number) = (line.newline, line.number);
            let end = start + lines.position().saturating_sub(prefix.len() as u64);
            if number == Some(current_number) {
                row.as_mut().expect("output row").terminated |= terminated;
                continue;
            }
            if let Some(row) = row.take() {
                self.push(row)?;
            }
            if end != group_end {
                group_start = group_end;
                group_end = end;
                ordinal = 0;
                group_literal = entering_literal;
                entering_literal = lines.in_view();
            } else {
                ordinal += 1;
            }
            number = Some(current_number);
            row = Some(Row {
                start: group_start,
                end,
                ordinal,
                codec: 0,
                state: [0; 3],
                terminated,
                literal: group_literal,
            });
        }
        if let Some(row) = row {
            self.push(row)?;
        }
        Ok(())
    }

    /// Only job outputs are append-only. Reparse the final unfinished group;
    /// a reparse that fails puts the index back as it was.
    pub fn grow(&mut self, bytes: u64, cancelled: Arc<AtomicBool>) -> Result<u64, NotGrown> {
        if !self.output || bytes < self.bytes {
            return Err(NotGrown::Ended("the source was rewritten or truncated".into()));
        }
        if bytes == self.bytes {
            return Ok(self.length);
        }
        let (length, before) = (self.length, self.bytes);
        let mut first = self.length;
        let mut start = self.bytes;
        let mut literal = false;
        if self.length > 0 {
            let last = self.row(self.length - 1).map_err(NotGrown::Passing)?;
            start = last.start;
            literal = last.literal;
            first -= 1;
            while first > 0 && self.row(first - 1).map_err(NotGrown::Passing)?.start == start {
                first -= 1;
            }
        }
        let kept = self.records(first, length).map_err(NotGrown::Passing)?;
        match self.reparse(first, start, literal, bytes, cancelled) {
            Ok(()) => Ok(first),
            Err(error) => match self.put_back(first, length, before, &kept) {
                Ok(()) => Err(NotGrown::Passing(error)),
                Err(lost) => Err(NotGrown::Ended(format!("{error}; the index could not be put back: {lost}"))),
            },
        }
    }

    /// Indexes the output again from `start` up to `bytes`, the rows from
    /// `first` on being those it gives.
    fn reparse(
        &mut self,
        first: u64,
        start: u64,
        literal: bool,
        bytes: u64,
        cancelled: Arc<AtomicBool>,
    ) -> Result<(), String> {
        self.rows.flush().map_err(err)?;
        self.rows.get_mut().file.set_len(first * RECORD).map_err(err)?;
        self.row_cache_start = None;
        self.file_cursor = None;
        self.length = first;
        self.bytes = bytes;
        self.append(start, literal, cancelled)
    }

    /// The records of rows `from..to` as they lie on disk.
    fn records(&mut self, from: u64, to: u64) -> Result<Vec<u8>, String> {
        self.rows.seek(SeekFrom::Start(from * RECORD)).map_err(err)?;
        let mut records = vec![0; ((to - from) * RECORD) as usize];
        self.rows.get_mut().file.read_exact(&mut records).map_err(err)?;
        self.row_cursor = to * RECORD;
        Ok(records)
    }

    /// Puts the index back as it was before a reparse that failed: `length`
    /// rows over `bytes` of the output, those from `first` on being `kept`.
    fn put_back(&mut self, first: u64, length: u64, bytes: u64, kept: &[u8]) -> Result<(), String> {
        // What the reparse left in the buffer is dropped, never written.
        self.rows.get_mut().dropping = true;
        let dropped = self.rows.flush();
        self.rows.get_mut().dropping = false;
        dropped.map_err(err)?;
        let file = &mut self.rows.get_mut().file;
        file.seek(SeekFrom::Start(first * RECORD)).map_err(err)?;
        file.write_all(kept).map_err(err)?;
        file.set_len(length * RECORD).map_err(err)?;
        self.row_cursor = length * RECORD;
        self.row_cache_start = None;
        self.file_cursor = None;
        self.length = length;
        self.bytes = bytes;
        Ok(())
    }

    pub fn read(&mut self, start: u64, count: u64, cancelled: &AtomicBool) -> Result<Vec<String>, String> {
        let mut result = Vec::new();
        let end = start.saturating_add(count).min(self.length);
        for line in start..end {
            check(cancelled)?;
            result.push(self.text(line, cancelled)?.text().map_err(err)?);
        }
        Ok(result)
    }

    /// Deliver requested rows in bounded UTF-8 pieces without collecting them.
    pub fn visit(
        &mut self,
        start: u64,
        count: u64,
        cancelled: &AtomicBool,
        mut emit: impl FnMut(u64, String, bool) -> Result<(), String>,
    ) -> Result<u64, String> {
        let end = start.saturating_add(count).min(self.length);
        for line in start..end {
            check(cancelled)?;
            let text = self.text(line, cancelled)?;
            let mut decoder = TextDecoder::new(TextEncoding::utf8());
            for chunk in text.chunks() {
                check(cancelled)?;
                let text = decoder.feed(&chunk.map_err(err)?, false).map_err(err)?;
                if !text.is_empty() {
                    emit(line, text, false)?;
                }
            }
            emit(line, decoder.feed(&[], true).map_err(err)?, true)?;
        }
        Ok(end.saturating_sub(start))
    }

    pub fn find(
        &mut self,
        query: &str,
        position: &Position,
        direction: Direction,
        cancelled: &AtomicBool,
    ) -> Result<Option<Match>, String> {
        if query.is_empty() {
            return Err("the literal query must not be empty".into());
        }
        if query.contains(['\n', '\r']) {
            return Err("the literal query must fit within one logical line".into());
        }
        let pattern = query.as_bytes();
        let mut prefix = vec![0; pattern.len()];
        for i in 1..pattern.len() {
            let mut n = prefix[i - 1];
            while n > 0 && pattern[i] != pattern[n] {
                n = prefix[n - 1];
            }
            if pattern[i] == pattern[n] {
                n += 1;
            }
            prefix[i] = n;
        }
        let length = query.encode_utf16().count() as u64;
        let mut line = position.line.min(self.length);
        if direction == Direction::Backward && line == self.length {
            if line == 0 {
                return Ok(None);
            }
            line -= 1;
        }
        while line < self.length {
            check(cancelled)?;
            let text = self.text(line, cancelled)?;
            let mut matched = 0;
            let mut utf16 = 0;
            let mut found = None;
            for chunk in text.chunks() {
                check(cancelled)?;
                for byte in chunk.map_err(err)? {
                    if byte & 0xc0 != 0x80 {
                        utf16 += if byte >= 0xf0 { 2 } else { 1 };
                    }
                    while matched > 0 && byte != pattern[matched] {
                        matched = prefix[matched - 1];
                    }
                    if byte == pattern[matched] {
                        matched += 1;
                    }
                    if matched == pattern.len() {
                        let offset = utf16 - length;
                        let eligible = line != position.line
                            || match direction {
                                Direction::Forward => offset >= position.offset.unwrap_or(0),
                                Direction::Backward => offset <= position.offset.unwrap_or(u64::MAX),
                            };
                        if eligible {
                            found = Some(Match { line, offset, length });
                            if direction == Direction::Forward {
                                return Ok(found);
                            }
                        }
                        matched = prefix[matched - 1];
                    }
                }
            }
            if found.is_some() {
                return Ok(found);
            }
            match direction {
                Direction::Forward => line += 1,
                Direction::Backward if line > 0 => line -= 1,
                _ => break,
            }
        }
        Ok(None)
    }

    fn text(&mut self, number: u64, cancelled: &AtomicBool) -> Result<Bytes, String> {
        let row = self.row(number)?;
        if self.file_cursor != Some(row.start) {
            self.file.seek(SeekFrom::Start(row.start)).map_err(err)?;
        }
        let mut raw = Spool::default();
        let mut input = (&mut self.file).take(row.end - row.start);
        let mut buffer = [0; BLOCK];
        loop {
            check(cancelled)?;
            let n = input.read(&mut buffer).map_err(err)?;
            if n == 0 {
                break;
            }
            raw.write_all(&buffer[..n]).map_err(err)?;
        }
        if raw.len() != row.end - row.start {
            return Err("text source ended during reading".into());
        }
        self.file_cursor = Some(row.end);
        let raw = raw.finish();
        if self.output {
            let prefix = if row.literal { crate::run::terminal::VIEW_MARK.as_bytes() } else { &[] };
            let mut lines = crate::run::terminal::Lines::new(std::io::Cursor::new(prefix).chain(raw.reader()));
            let mut ordinal = 0;
            let mut number = None;
            let mut shown = Spool::default();
            while let Some(line) = lines.next_line().map_err(err)? {
                check(cancelled)?;
                if number != Some(line.number) {
                    if number.is_some() {
                        if ordinal == row.ordinal {
                            return Ok(shown.finish());
                        }
                        ordinal += 1;
                    }
                    number = Some(line.number);
                }
                if ordinal == row.ordinal {
                    line.parts.write_text(&mut shown).map_err(err)?;
                }
            }
            return if number.is_some() && ordinal == row.ordinal {
                Ok(shown.finish())
            } else {
                Err("output checkpoint has no row".into())
            };
        }
        let decoded = if row.codec == 0 {
            let ending = if raw.ends_with(&self.crlf).map_err(err)? {
                self.crlf.len()
            } else if raw.ends_with(&self.newline).map_err(err)? {
                self.newline.len()
            } else {
                0
            };
            encoding::line::preview(&raw.slice(0..raw.len() - ending), true, "")?
        } else {
            let codec = self.codecs[row.codec as usize - 1];
            let mut decoder = TextDecoder::new(codec);
            // Consecutive ISO-2022-JP escapes can be strictly invalid. An
            // initial ASCII state or the row's own switch needs no prefix.
            if codec.is_stateful() && row.state != *b"\x1b(B" && raw.byte(0).map_err(err)? != Some(0x1b) {
                decoder.feed(&row.state, false).map_err(err)?;
            }
            encoding::line::decode(&raw, &mut decoder, row.end == self.bytes)?.map_err(err)?
        };
        let ending = if decoded.ends_with(b"\r\n").map_err(err)? {
            2
        } else if decoded.ends_with(b"\n").map_err(err)? {
            1
        } else {
            0
        };
        let content = decoded.slice(0..decoded.len() - ending);
        let mut shown = Spool::default();
        let mut decoder = TextDecoder::new(TextEncoding::utf8());
        for chunk in content.chunks() {
            check(cancelled)?;
            let text = decoder.feed(&chunk.map_err(err)?, false).map_err(err)?;
            shown.write_all(crate::text::visible(&text).as_bytes()).map_err(err)?;
        }
        shown.write_all(crate::text::visible(&decoder.feed(&[], true).map_err(err)?).as_bytes()).map_err(err)?;
        Ok(shown.finish())
    }

    fn push(&mut self, row: Row) -> Result<(), String> {
        if self.row_cursor != self.length * RECORD {
            self.rows.seek(SeekFrom::Start(self.length * RECORD)).map_err(err)?;
        }
        for value in [row.start, row.end, row.ordinal] {
            self.rows.write_all(&value.to_le_bytes()).map_err(err)?;
        }
        self.rows.write_all(&row.codec.to_le_bytes()).map_err(err)?;
        self.rows.write_all(&row.state).map_err(err)?;
        self.rows.write_all(&[u8::from(row.terminated), u8::from(row.literal)]).map_err(err)?;
        self.rows.write_all(&[0; 7]).map_err(err)?;
        self.length += 1;
        self.row_cursor = self.length * RECORD;
        Ok(())
    }

    fn row(&mut self, number: u64) -> Result<Row, String> {
        let per_block = BLOCK as u64 / RECORD;
        let start = number / per_block * per_block;
        if self.row_cache_start != Some(start) {
            self.rows.seek(SeekFrom::Start(start * RECORD)).map_err(err)?;
            let count = (self.length - start).min(per_block);
            self.row_cache.resize((count * RECORD) as usize, 0);
            self.rows.get_mut().file.read_exact(&mut self.row_cache).map_err(err)?;
            self.row_cursor = (start + count) * RECORD;
            self.row_cache_start = Some(start);
        }
        let at = ((number - start) * RECORD) as usize;
        let bytes = &self.row_cache[at..at + RECORD as usize];
        let word = |at| u64::from_le_bytes(bytes[at..at + 8].try_into().expect("word"));
        Ok(Row {
            start: word(0),
            end: word(8),
            ordinal: word(16),
            codec: u32::from_le_bytes(bytes[24..28].try_into().expect("codec")),
            state: bytes[28..31].try_into().expect("state"),
            terminated: bytes[31] != 0,
            literal: bytes[32] != 0,
        })
    }
}

struct Input {
    input: std::io::Take<File>,
    cancelled: Arc<AtomicBool>,
}
impl Input {
    fn new(file: File, bytes: u64, cancelled: Arc<AtomicBool>) -> Self {
        Self { input: file.take(bytes), cancelled }
    }
}
impl Read for Input {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        check(&self.cancelled).map_err(io::Error::other)?;
        self.input.read(bytes)
    }
}
fn check(cancelled: &AtomicBool) -> Result<(), String> {
    if cancelled.load(Ordering::Relaxed) { Err("line request was cancelled".into()) } else { Ok(()) }
}
fn err(error: impl std::fmt::Display) -> String {
    error.to_string()
}
