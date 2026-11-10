//! Reading a file and listing a directory for a look: a file whole, or its
//! last lines, or the lines that hold some words; a directory's entries with
//! their size and when they changed. Only regular files are read, never more
//! of one than a look can show, and what is shown is held to what one look
//! gives back, the latest kept.

use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::Path;

use fastctx_types::inspect::{Inspected, MOST_BYTES, MOST_LINES};
use fastctx_types::ipc::Failure;

use super::guard::{Guard, Opened};
use super::noted;

/// How far back from its end a file is searched for words.
const SEARCHED: u64 = 16 * 1024 * 1024;
/// How far back from its end a file is read for its last lines.
const TAILED: u64 = 1024 * 1024;
/// How much of a file too large to show whole is shown from its start.
const HEAD: usize = 8 * 1024;
/// The most entries a listing shows.
const ENTRIES: usize = 1_000;

/// The file at `path`, as `tail` or `matching` ask, or whole.
pub(super) fn read(guard: &Guard, path: &str, tail: Option<u32>, matching: Option<&str>) -> Result<Inspected, Failure> {
    match guard.open(path)? {
        Opened::Missing => Ok(noted(format!("no such file: {path}"))),
        Opened::Found(file) if fastctx_platform::disk::is_dir(&file) => {
            Ok(noted(format!("{path} is a directory; list it instead")))
        }
        Opened::Found(file) => read_at(&file, path, tail, matching),
    }
}

/// The file at `file`, called `shown`, read as `read` reads it.
pub(super) fn read_at(
    file: &Path,
    shown: &str,
    tail: Option<u32>,
    matching: Option<&str>,
) -> Result<Inspected, Failure> {
    let failed = |error: std::io::Error| Failure::not_run(format!("{shown}: {error}"));
    let mut opened = match fastctx_platform::disk::open_regular(file) {
        Ok(opened) => opened,
        Err(error) if error.kind() == std::io::ErrorKind::InvalidInput => {
            return Ok(noted(format!(
                "{shown} is not a regular file but a device, a pipe or a socket, which a look does not read"
            )));
        }
        Err(error) => return Err(failed(error)),
    };
    let size = opened.metadata().map_err(failed)?.len();
    let start = bytes_from(&mut opened, 0, HEAD as u64).map_err(failed)?;
    let kind = Kind::of(&start);
    if kind == Kind::Bytes && start.contains(&0) {
        return Ok(noted(format!("{shown} is a binary file of {size} bytes, not shown")));
    }
    if let Some(words) = matching {
        let from = size.saturating_sub(SEARCHED);
        let text = text_from(&mut opened, from, SEARCHED, kind).map_err(failed)?;
        return Ok(holding(&text, from > 0, words, size));
    }
    if let Some(lines) = tail {
        let from = size.saturating_sub(TAILED);
        let text = text_from(&mut opened, from, TAILED, kind).map_err(failed)?;
        return Ok(last_lines(&text, lines));
    }
    if size as usize <= MOST_BYTES {
        let text = text_from(&mut opened, 0, MOST_BYTES as u64, kind).map_err(failed)?;
        return Ok(Inspected { text, note: None });
    }
    let head = kind.decode(kind.whole_lines(&start));
    let from = size - (MOST_BYTES - HEAD - 128) as u64;
    let end = text_from(&mut opened, from, MOST_BYTES as u64, kind).map_err(failed)?;
    Ok(Inspected {
        text: format!("{head}…\n{end}"),
        note: Some(format!(
            "{shown} holds {size} bytes; its start and its end are shown, the {} between them are not",
            from - HEAD as u64
        )),
    })
}

/// The entries of the directory at `path`, or those whose names fit
/// `pattern`.
pub(super) fn list(guard: &Guard, path: &str, pattern: Option<&str>) -> Result<Inspected, Failure> {
    let directory = match guard.open(path)? {
        Opened::Missing => return Ok(noted(format!("no such directory: {path}"))),
        Opened::Found(directory) if !fastctx_platform::disk::is_dir(&directory) => {
            return Ok(noted(format!("{path} is a file; read it instead")));
        }
        Opened::Found(directory) => directory,
    };
    let failed = |error: std::io::Error| Failure::not_run(format!("{path}: {error}"));
    let mut entries = Vec::new();
    for entry in fastctx_platform::disk::read_dir(&directory).map_err(failed)? {
        let entry = entry.map_err(failed)?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if pattern.is_some_and(|pattern| !fits(&name.to_lowercase(), &pattern.to_lowercase())) {
            continue;
        }
        let Ok(metadata) = entry.metadata() else { continue };
        let kind = if metadata.is_dir() {
            "directory"
        } else if metadata.file_type().is_symlink() {
            "link"
        } else {
            "file"
        };
        let size = if metadata.is_dir() { String::new() } else { format!("{} bytes", metadata.len()) };
        let changed = metadata.modified().ok().and_then(|at| jiff::Timestamp::try_from(at).ok());
        entries.push((name, kind, size, changed));
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    if entries.is_empty() {
        return Ok(noted(match pattern {
            Some(pattern) => format!("nothing in {path} fits {pattern}"),
            None => format!("{path} is empty"),
        }));
    }
    let more = entries.len().saturating_sub(ENTRIES);
    let text: String = entries
        .iter()
        .take(ENTRIES)
        .map(|(name, kind, size, changed)| {
            let changed = changed.map(super::machine::moment).unwrap_or_default();
            format!("{name}  {kind}  {size}  {changed}\n")
        })
        .collect();
    let note = (more > 0).then(|| format!("{} entries; the first {ENTRIES} by name are shown", entries.len()));
    Ok(Inspected { text, note })
}

/// Whether `name` fits `pattern`, where `*` stands for any run of
/// characters and `?` for one.
fn fits(name: &str, pattern: &str) -> bool {
    let (name, pattern): (Vec<char>, Vec<char>) = (name.chars().collect(), pattern.chars().collect());
    let (mut at, mut from) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while at < name.len() {
        if from < pattern.len() && (pattern[from] == '?' || pattern[from] == name[at]) {
            at += 1;
            from += 1;
        } else if from < pattern.len() && pattern[from] == '*' {
            star = Some((from, at));
            from += 1;
        } else if let Some((star_from, star_at)) = star {
            from = star_from + 1;
            at = star_at + 1;
            star = Some((star_from, star_at + 1));
        } else {
            return false;
        }
    }
    pattern[from..].iter().all(|&character| character == '*')
}

/// At most `limit` bytes of a file from `offset` on.
fn bytes_from(file: &mut std::fs::File, offset: u64, limit: u64) -> std::io::Result<Vec<u8>> {
    file.seek(SeekFrom::Start(offset))?;
    let mut bytes = Vec::new();
    file.take(limit).read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// At most `limit` bytes of a file from `offset` on, as text: from partway
/// into the file, its first line, which starts before what was read, is
/// left out.
fn text_from(file: &mut std::fs::File, offset: u64, limit: u64, kind: Kind) -> std::io::Result<String> {
    let offset = kind.aligned(offset);
    let bytes = bytes_from(file, offset, limit)?;
    let bytes = if offset > 0 { kind.after_first_line(&bytes) } else { &bytes[..] };
    Ok(kind.decode(bytes))
}

/// How a file's bytes are text: UTF-16 as its byte order mark says, or
/// bytes whose encoding they show themselves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Utf16Le,
    Utf16Be,
    Bytes,
}

impl Kind {
    /// The kind of the file that starts with `start`.
    fn of(start: &[u8]) -> Self {
        if start.starts_with(&[0xff, 0xfe]) {
            Self::Utf16Le
        } else if start.starts_with(&[0xfe, 0xff]) {
            Self::Utf16Be
        } else {
            Self::Bytes
        }
    }

    /// `offset` moved onto the start of a character's code unit.
    fn aligned(self, offset: u64) -> u64 {
        match self {
            Self::Bytes => offset,
            Self::Utf16Le | Self::Utf16Be => offset + offset % 2,
        }
    }

    /// How many bytes a code unit takes.
    fn width(self) -> usize {
        if self == Self::Bytes { 1 } else { 2 }
    }

    /// Where the whole code units of `bytes` start.
    fn units(self, bytes: &[u8]) -> std::iter::StepBy<std::ops::Range<usize>> {
        let width = self.width();
        (0..bytes.len() / width * width).step_by(width)
    }

    /// Whether the code unit at `at` ends a line: a line feed, or a carriage
    /// return, which a progress bar ends each of its updates with.
    fn ends_line(self, bytes: &[u8], at: usize) -> bool {
        let unit = match self {
            Self::Bytes => bytes[at],
            Self::Utf16Le if bytes[at + 1] == 0 => bytes[at],
            Self::Utf16Be if bytes[at] == 0 => bytes[at + 1],
            _ => return false,
        };
        unit == b'\n' || unit == b'\r'
    }

    /// `bytes` from after the end of their first line on; where they end no
    /// line, all of them from their first whole character, the end of one
    /// line too long to read whole.
    fn after_first_line(self, bytes: &[u8]) -> &[u8] {
        let mut units = self.units(bytes);
        match units.find(|at| self.ends_line(bytes, *at)) {
            Some(at) => &bytes[at + self.width()..],
            None if self == Self::Bytes => {
                let partial = bytes.iter().take(3).take_while(|byte| *byte & 0xc0 == 0x80).count();
                &bytes[partial..]
            }
            None => bytes,
        }
    }

    /// `bytes` up to and with the end of their last line: the lines a cut
    /// leaves whole.
    fn whole_lines(self, bytes: &[u8]) -> &[u8] {
        let mut units = self.units(bytes).rev();
        units.find(|at| self.ends_line(bytes, *at)).map_or(bytes, |at| &bytes[..at + self.width()])
    }

    /// `bytes` as text, what does not read replaced.
    fn decode(self, bytes: &[u8]) -> String {
        let pair = |unit: &[u8]| [unit[0], unit[1]];
        let units: Vec<u16> = match self {
            Self::Bytes => return text_of(bytes),
            Self::Utf16Le => bytes.chunks_exact(2).map(|unit| u16::from_le_bytes(pair(unit))).collect(),
            Self::Utf16Be => bytes.chunks_exact(2).map(|unit| u16::from_be_bytes(pair(unit))).collect(),
        };
        let units = units.strip_prefix(&[0xfeff]).unwrap_or(&units);
        char::decode_utf16(units.iter().copied()).map(|unit| unit.unwrap_or(char::REPLACEMENT_CHARACTER)).collect()
    }
}

/// Bytes as text: UTF-8, its last character cut short left out; or the
/// encoding they are found to be in, the likeliest where several could be;
/// or UTF-8 with what does not read replaced.
fn text_of(bytes: &[u8]) -> String {
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    match std::str::from_utf8(bytes) {
        Ok(text) => return text.to_string(),
        Err(error) if error.error_len().is_none() => {
            return String::from_utf8_lossy(&bytes[..error.valid_up_to()]).into_owned();
        }
        Err(_) => {}
    }
    use fastctx_hands::encoding::{EncodingError, detect};
    let encoding = match detect(bytes, None) {
        Ok(encoding) => Some(encoding),
        Err(EncodingError::Ambiguous { candidates, .. }) => {
            candidates.first().and_then(|likeliest| detect(bytes, Some(likeliest)).ok())
        }
        Err(_) => None,
    };
    encoding
        .and_then(|encoding| encoding.decode(bytes).ok())
        .unwrap_or_else(|| String::from_utf8_lossy(bytes).into_owned())
}

/// The lines of `text`, those a progress bar ends with a carriage return
/// among them, blank ones left out.
fn lines(text: &str) -> Vec<&str> {
    text.split(['\n', '\r']).filter(|line| !line.trim().is_empty()).collect()
}

/// The last `count` lines of `text`.
fn last_lines(text: &str, count: u32) -> Inspected {
    let lines = lines(text);
    let start = lines.len().saturating_sub(count.min(MOST_LINES) as usize);
    latest(&lines[start..])
}

/// The lines of `text` that hold `words`, whatever their case; `cut` when
/// only the end of the file, `size` bytes in all, was read.
fn holding(text: &str, cut: bool, words: &str, size: u64) -> Inspected {
    let wanted = words.to_lowercase();
    let found: Vec<&str> = lines(text).into_iter().filter(|line| line.to_lowercase().contains(&wanted)).collect();
    let mut notes = Vec::new();
    let start = found.len().saturating_sub(MOST_LINES as usize);
    let mut seen = latest(&found[start..]);
    if found.is_empty() {
        notes.push(format!("no line holds “{words}”"));
    } else if start > 0 {
        notes.push(format!("{} lines hold “{words}”; the last {MOST_LINES} are shown", found.len()));
    }
    notes.extend(seen.note.take());
    if cut {
        notes.push(format!("only the last {} MiB of its {size} bytes were searched", SEARCHED / 1024 / 1024));
    }
    seen.note = (!notes.is_empty()).then(|| notes.join("; "));
    seen
}

/// `lines` within what one look gives back, the latest kept; of a last line
/// too long for it alone, its end.
fn latest(lines: &[&str]) -> Inspected {
    if let Some(newest) = lines.last()
        && newest.len() + 1 > MOST_BYTES - 256
    {
        let mut start = newest.len() - (MOST_BYTES - 256);
        while !newest.is_char_boundary(start) {
            start += 1;
        }
        return Inspected {
            text: format!("…{}\n", &newest[start..]),
            note: Some(format!("the last line holds {} bytes; its end is shown", newest.len())),
        };
    }
    let mut kept = Vec::new();
    let mut bytes = 0;
    for line in lines.iter().rev() {
        bytes += line.len() + 1;
        if bytes > MOST_BYTES - 256 {
            break;
        }
        kept.push(*line);
    }
    let mut text = String::new();
    for line in kept.iter().rev() {
        text.push_str(line);
        text.push('\n');
    }
    let note = (kept.len() < lines.len())
        .then(|| format!("the {} earlier lines would not fit and are not shown", lines.len() - kept.len()));
    Inspected { text, note }
}
