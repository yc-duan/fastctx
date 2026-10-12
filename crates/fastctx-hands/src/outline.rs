//! A folder of this machine at a glance, for a model told where its work is
//! (`world-gui.md`, 新对话): what is in it, one level only, each folder with
//! how many entries it holds and each file with how many lines, or its size
//! where it is not text. Reading stops at a deadline, and whatever was not
//! read by then is marked so, a file still with its size.

use std::fs;
use std::io::Read as _;
use std::path::Path;
use std::time::Instant;

use crate::run::present::display_path;

/// At most this many entries are named, folders first: a project's top
/// holds far fewer, and past a hundred a folder is mostly data whose names
/// say little. The rest are counted.
const NAMED: usize = 100;

/// How much of a file is read at once while counting its lines.
const CHUNK: usize = 64 * 1024;

struct Entry {
    name: String,
    folder: bool,
    size: u64,
    /// Entries in a folder, lines in a text file; `None` until known.
    count: Option<Counted>,
}

enum Counted {
    Entries(usize),
    Lines(u64),
    /// Not text: only its size is told.
    Binary,
    Failed(String),
}

/// `path` with what is in it; why it could not be read at all.
pub fn outline(path: &str, deadline: Instant) -> Result<String, String> {
    let root = Path::new(path);
    if !root.is_absolute() {
        return Err(format!("{path} is not an absolute path"));
    }
    if !root.is_dir() {
        return Err(if root.exists() { "it is not a folder".to_string() } else { "it does not exist".to_string() });
    }
    let mut entries = Vec::new();
    for entry in fs::read_dir(root).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let name = entry.file_name().to_string_lossy().into_owned();
        // A link is told as what it points to.
        let metadata = fs::metadata(entry.path()).or_else(|_| entry.metadata());
        let (folder, size) = metadata.map(|metadata| (metadata.is_dir(), metadata.len())).unwrap_or((false, 0));
        entries.push(Entry { name, folder, size, count: None });
    }
    entries.sort_by(|a, b| {
        b.folder.cmp(&a.folder).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())).then(a.name.cmp(&b.name))
    });
    let rest = entries.split_off(entries.len().min(NAMED));
    let mut stopped = false;
    // Folders first, then files from the smallest: a huge file is counted
    // last, so it never keeps the others from being counted.
    for entry in entries.iter_mut().filter(|entry| entry.folder) {
        if Instant::now() >= deadline {
            stopped = true;
            break;
        }
        entry.count = Some(match fs::read_dir(root.join(&entry.name)) {
            Ok(inside) => Counted::Entries(inside.count()),
            Err(error) => Counted::Failed(error.to_string()),
        });
    }
    let mut files: Vec<&mut Entry> = entries.iter_mut().filter(|entry| !entry.folder).collect();
    files.sort_by_key(|entry| entry.size);
    for entry in files {
        if stopped || Instant::now() >= deadline {
            stopped = true;
            break;
        }
        entry.count = lines(&root.join(&entry.name), deadline);
        stopped = entry.count.is_none();
    }
    let mut text = display_path(path);
    if entries.is_empty() {
        text.push_str(" (empty)");
        return Ok(text);
    }
    let folders: Vec<String> = entries.iter().filter(|entry| entry.folder).map(told).collect();
    let files: Vec<String> = entries.iter().filter(|entry| !entry.folder).map(told).collect();
    if !folders.is_empty() {
        text.push_str(&format!("\n  folders: {}", folders.join(", ")));
    }
    if !files.is_empty() {
        text.push_str(&format!("\n  files: {}", files.join(", ")));
    }
    if !rest.is_empty() {
        let more_folders = rest.iter().filter(|entry| entry.folder).count();
        text.push_str(&format!(
            "\n  and {} more: {} folders, {} files",
            grouped(rest.len() as u64),
            grouped(more_folders as u64),
            grouped((rest.len() - more_folders) as u64)
        ));
    }
    if stopped {
        text.push_str("\n  (reading stopped at its time limit)");
    }
    Ok(text)
}

/// The lines of the text file at `path`, as `fastctx view` numbers them;
/// none when the deadline came first.
fn lines(path: &Path, deadline: Instant) -> Option<Counted> {
    let mut file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) => return Some(Counted::Failed(error.to_string())),
    };
    let mut buffer = vec![0u8; CHUNK];
    let mut newlines = 0u64;
    let mut last = None;
    let mut first = true;
    // UTF-16 with its byte order mark counts its own newline unit; a NUL
    // anywhere else in the first chunk says the file is not text.
    let mut wide: Option<[u8; 2]> = None;
    let mut odd = None;
    loop {
        if Instant::now() >= deadline {
            return None;
        }
        let read = match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Some(Counted::Failed(error.to_string())),
        };
        let mut bytes = &buffer[..read];
        if first {
            first = false;
            if bytes.starts_with(&[0xff, 0xfe]) {
                wide = Some([b'\n', 0]);
                bytes = &bytes[2..];
            } else if bytes.starts_with(&[0xfe, 0xff]) {
                wide = Some([0, b'\n']);
                bytes = &bytes[2..];
            } else if memchr::memchr(0, bytes).is_some() {
                return Some(Counted::Binary);
            }
        }
        match wide {
            None => newlines += memchr::memchr_iter(b'\n', bytes).count() as u64,
            Some(newline) => {
                // A unit may straddle two reads.
                let mut units = Vec::with_capacity(bytes.len() + 1);
                units.extend(odd.take());
                units.extend_from_slice(bytes);
                if units.len() % 2 == 1 {
                    odd = units.pop();
                }
                newlines += units.chunks_exact(2).filter(|unit| *unit == newline).count() as u64;
                last = units.chunks_exact(2).last().map(|unit| unit == newline);
                continue;
            }
        }
        last = bytes.last().map(|byte| *byte == b'\n');
    }
    Some(Counted::Lines(match last {
        None => 0,
        Some(true) => newlines,
        Some(false) => newlines + 1,
    }))
}

fn told(entry: &Entry) -> String {
    match &entry.count {
        None if entry.folder => format!("{} (not read)", entry.name),
        None => format!("{} ({}, not read)", entry.name, size(entry.size)),
        Some(Counted::Binary) => format!("{} ({})", entry.name, size(entry.size)),
        Some(Counted::Entries(entries)) => format!("{} ({})", entry.name, grouped(*entries as u64)),
        Some(Counted::Lines(0)) => format!("{} (empty)", entry.name),
        Some(Counted::Lines(1)) => format!("{} (1 line)", entry.name),
        Some(Counted::Lines(lines)) => format!("{} ({} lines)", entry.name, grouped(*lines)),
        Some(Counted::Failed(why)) => format!("{} (could not be read: {why})", entry.name),
    }
}

/// A size as people say it: `512 B`, `18 KB`, `1.2 MB`.
fn size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 || value >= 10.0 { format!("{value:.0} {}", UNITS[unit]) } else { format!("{value:.1} {}", UNITS[unit]) }
}

/// A count with thousands set apart: `48,210`.
fn grouped(number: u64) -> String {
    let digits = number.to_string();
    let mut text = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            text.push(',');
        }
        text.push(digit);
    }
    text
}
