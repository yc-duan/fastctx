//! `fastctx view`: looking at files (`world-view.md`).
//!
//! Paths are shown in the order given, each under a head note saying what
//! it is and how much of it is shown, until the answer's budget is spent;
//! the files after that are named with their size and not shown. Nothing is
//! cut silently: a file shown in part says where it stops, and when any file
//! is not shown whole the first line sums up what was and was not. Text is
//! shown as `number<TAB>line`. Images are written in the iTerm2 terminal
//! protocol, which `run` turns back into images.

mod decode;
mod office;
mod pdf;
mod text;

pub(crate) use text::visible;

use std::fs::File;
use std::io::{self, Read as _, Write};
use std::path::{Path, PathBuf};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use fastctx_types::residents::Seen;
use jiff::Timestamp;
use sha2::{Digest as _, Sha256};

use crate::image::sniff;
use crate::run::terminal::{VIEW_END, VIEW_MARK};
use crate::run::window::{BUDGET, IMAGE_COST, RUN_NOTES, bytes};

/// How to look.
#[derive(Clone, Debug, Default)]
pub struct Options {
    /// Lines `first..=last`, counting from 1, of every text file.
    pub lines: Option<(u64, u64)>,
    /// Characters `first..=last` of each line shown, counting from 1 as the
    /// head note counts them: for reading on in a line too long to show.
    pub chars: Option<(u64, u64)>,
    /// PDF pages or PowerPoint slides `first..=last`, counting from 1.
    pub pages: Option<(u64, u64)>,
    /// Only the master and layout text of a PowerPoint presentation.
    pub master: bool,
    /// Excel worksheet name.
    pub sheet: Option<String>,
    /// Excel worksheet index, counting from one.
    pub sheet_index: Option<usize>,
    /// Excel rectangle in A1 notation.
    pub range: Option<fastctx_office::CellRange>,
    /// Render PDF pages instead of extracting their text layer.
    pub image: bool,
    /// An explicit codec or ordered per-line codec list. Labels follow WHATWG; GBK uses GB18030.
    pub encoding: Option<String>,
    /// The machine's name, to say where each file is.
    pub machine: Option<String>,
    /// Inside a World `run`, whose first line already names the machine and
    /// whose answer holds the view beside its own notes.
    pub in_run: bool,
}

/// Shows `paths` on `out` and returns what was seen of each file shown.
pub fn view(paths: &[PathBuf], options: &Options, out: &mut impl Write) -> io::Result<Vec<Seen>> {
    // The summary and the note each file gets when it is not shown are paid
    // for first: whatever happens, every path is named.
    let files = plural(paths.len() as u64, "file", "files");
    let summary_at_longest = format!("=== view ({files}: {n} shown, {n} not shown) ===\n", n = paths.len());
    let notes: Vec<Section> = paths.iter().map(|path| not_shown(path, options)).collect();
    // Costs here are UTF-8 bytes, four to each unit of the answer's budget.
    let budget = bytes(if options.in_run { BUDGET - RUN_NOTES } else { BUDGET });
    let mut left = budget.saturating_sub(summary_at_longest.len() + notes.iter().map(|note| note.cost).sum::<usize>());
    let mut sections = Vec::new();
    let mut seen = Vec::new();
    let mut stopped = false;
    for (path, note) in paths.iter().zip(notes) {
        let section = if stopped {
            note
        } else {
            // This file's own note is no longer needed if it is shown.
            let (section, observed) = look(path, options, left + note.cost)?;
            if section.partial {
                stopped = true;
            }
            left = (left + note.cost).saturating_sub(section.cost);
            seen.extend(observed);
            section
        };
        sections.push(section);
    }
    let shown = sections.iter().filter(|section| section.shown).count();
    if options.in_run {
        out.write_all(VIEW_MARK.as_bytes())?;
    }
    if shown < paths.len() {
        writeln!(out, "=== view ({files}: {shown} shown, {} not shown) ===", paths.len() - shown)?;
    }
    for section in sections {
        out.write_all(section.text.as_bytes())?;
    }
    if options.in_run {
        out.write_all(VIEW_END.as_bytes())?;
    }
    Ok(seen)
}

/// One path's part of the answer.
struct Section {
    text: String,
    /// What it costs against the budget, in bytes, an image at its fixed
    /// cost.
    cost: usize,
    /// Whether any of the file's content is shown.
    shown: bool,
    /// Whether the budget ran out in it, so later files are not shown.
    partial: bool,
}

impl Section {
    fn note(path: &Path, options: &Options, facts: &str) -> Self {
        let text = format!("{}\n", head(path, options, facts));
        Self { cost: text.len(), text, shown: false, partial: false }
    }
}

fn head(path: &Path, options: &Options, facts: &str) -> String {
    let path = path.to_string_lossy().replace('\\', "/");
    match &options.machine {
        Some(machine) => format!("=== {machine}:{path} ({facts}) ==="),
        None => format!("=== {path} ({facts}) ==="),
    }
}

fn not_shown(path: &Path, options: &Options) -> Section {
    let native = match fastctx_platform::local_path(path) {
        Ok(path) => path,
        Err(error) => return Section::note(path, options, &describe_error(&error)),
    };
    let facts = match std::fs::metadata(&native) {
        Ok(metadata) => format!("not shown: {}", size(metadata.len())),
        Err(error) => describe_error(&error),
    };
    Section::note(&native, options, &facts)
}

fn look(path: &Path, options: &Options, budget: usize) -> io::Result<(Section, Option<Seen>)> {
    let native = match fastctx_platform::local_path(path) {
        Ok(path) => path,
        Err(error) => return Ok((Section::note(path, options, &describe_error(&error)), None)),
    };
    let resolved = match std::path::absolute(&native).and_then(|path| crate::files::canonical_path(&path)) {
        Ok(path) => path,
        Err(error) => return Ok((Section::note(&native, options, &describe_error(&error)), None)),
    };
    let (section, mut seen) = look_file(&native, &resolved, options, budget)?;
    if let Some(seen) = &mut seen {
        seen.path = resolved.to_string_lossy().into_owned();
    }
    Ok((section, seen))
}

fn look_file(path: &Path, resolved: &Path, options: &Options, budget: usize) -> io::Result<(Section, Option<Seen>)> {
    let metadata = match std::fs::metadata(resolved) {
        Ok(metadata) => metadata,
        Err(error) => return Ok((Section::note(path, options, &describe_error(&error)), None)),
    };
    if metadata.is_dir() {
        return Ok((Section::note(path, options, "a directory; list it with ls or find"), None));
    }
    if !metadata.is_file() {
        // A device, a FIFO or a socket could make looking wait forever.
        return Ok((Section::note(path, options, "a special file, not shown"), None));
    }
    let mut file = match File::open(resolved) {
        Ok(file) => file,
        Err(error) => return Ok((Section::note(path, options, &describe_error(&error)), None)),
    };
    let mut start = vec![0u8; 512];
    let read = read_up_to(&mut file, &mut start)?;
    start.truncate(read);
    let detected_image = sniff(&start);
    if let Some(format) = fastctx_office::format(path) {
        let result = office::look(path, options, budget, file, &metadata, format)?;
        if result.0.cost > budget {
            let mut note = not_shown(path, options);
            note.partial = true;
            return Ok((note, None));
        }
        return Ok(result);
    }
    if options.master {
        return Ok((Section::note(path, options, "--master applies to PowerPoint presentations"), None));
    }
    if options.sheet.is_some() || options.sheet_index.is_some() || options.range.is_some() {
        return Ok((Section::note(path, options, "--sheet and --range apply to Excel workbooks"), None));
    }
    if start.starts_with(b"%PDF-")
        || (detected_image.is_none() && path.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("pdf")))
    {
        let result = pdf::look(path, options, budget, file, &metadata)?;
        if result.0.cost > budget {
            let mut note = not_shown(path, options);
            note.partial = true;
            return Ok((note, None));
        }
        return Ok(result);
    }
    if options.pages.is_some() || options.image {
        return Ok((
            Section::note(path, options, "--pages selects PDF pages or PowerPoint slides; --image renders PDF pages"),
            None,
        ));
    }
    if let Some((mime, width, height)) = detected_image {
        return image(path, options, budget, file, start, mime, width, height, metadata.len());
    }
    text::look(path, options, budget, file, start, &metadata)
}

#[allow(clippy::too_many_arguments)]
fn image(
    path: &Path,
    options: &Options,
    budget: usize,
    mut file: File,
    start: Vec<u8>,
    mime: &str,
    width: u32,
    height: u32,
    length: u64,
) -> io::Result<(Section, Option<Seen>)> {
    // More than one answer can carry is never read in: shrink it first.
    if length > crate::run::window::IMAGE_BYTES as u64 {
        let facts = format!(
            "not shown: image {width}x{height}, {}, more than an answer can carry; make a smaller copy to look at",
            size(length)
        );
        return Ok((Section::note(path, options, &facts), None));
    }
    let note = format!("{}\n", head(path, options, &format!("image {width}x{height}, {mime}")));
    let cost = note.len() + bytes(IMAGE_COST);
    if cost > budget {
        let mut section = Section::note(path, options, &format!("not shown: image {width}x{height}, {}", size(length)));
        section.partial = true;
        return Ok((section, None));
    }
    let mut bytes = start;
    file.read_to_end(&mut bytes)?;
    let seen = Seen {
        path: path.to_string_lossy().into_owned(),
        at: Timestamp::now(),
        sha256: Some(hex(&Sha256::digest(&bytes))),
        size: bytes.len() as u64,
        modified: None,
        encoding: None,
        encoding_selection: None,
    };
    let text = format!("{note}\x1b]1337;File=inline=1;size={}:{}\x07\n", bytes.len(), STANDARD.encode(&bytes));
    Ok((Section { text, cost, shown: true, partial: false }, Some(seen)))
}

fn read_up_to(file: &mut File, buffer: &mut [u8]) -> io::Result<usize> {
    let mut filled = 0;
    while filled < buffer.len() {
        match file.read(&mut buffer[filled..])? {
            0 => break,
            read => filled += read,
        }
    }
    Ok(filled)
}

fn describe_error(error: &io::Error) -> String {
    match error.kind() {
        io::ErrorKind::NotFound => "no such file".to_string(),
        io::ErrorKind::PermissionDenied => format!("not readable: {error}"),
        _ => format!("could not be read: {error}"),
    }
}

/// `count` with the noun in the number it takes.
fn plural(count: u64, one: &str, many: &str) -> String {
    if count == 1 { format!("1 {one}") } else { format!("{count} {many}") }
}

/// A size in the unit a person reads it in.
fn size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1000 {
        return format!("{bytes} bytes");
    }
    let mut value = bytes as f64;
    let mut unit = "bytes";
    for next in UNITS {
        if value < 1000.0 {
            break;
        }
        value /= 1000.0;
        unit = next;
    }
    format!("{value:.1} {unit}")
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
