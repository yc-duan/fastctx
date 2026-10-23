//! `fastctx view`: looking at files (`world-view.md`).
//!
//! Paths are shown in the order given, each under a head note saying what
//! it is and how much of it is shown, until the answer's budget is spent;
//! the files after that are named with their size and not shown. Nothing is
//! cut silently: a file shown in part says where it stops, and when any file
//! is not shown whole the first line sums up what was and was not. Text is
//! shown as `number<TAB>line`. Images are written in the iTerm2 terminal
//! protocol, which `run` turns back into images.

mod config_outline;
mod decode;
mod definitions;
mod explore;
mod javascript;
mod office;
mod outline;
mod pdf;
mod text;

pub(crate) use text::visible;

use std::fs::File;
use std::io::{self, Read as _, Seek as _, Write};
use std::path::{Path, PathBuf};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use fastctx_types::residents::Seen;
use jiff::Timestamp;
use sha2::{Digest as _, Sha256};

use crate::image::Header;
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

impl Options {
    fn exploring(&self) -> bool {
        self.lines.is_none()
            && self.chars.is_none()
            && self.pages.is_none()
            && self.sheet.is_none()
            && self.sheet_index.is_none()
            && self.range.is_none()
            && !self.master
    }
}

/// The observations retained even when another requested file fails.
pub struct Viewed {
    pub seen: Vec<Seen>,
    pub failed: bool,
    /// The pictures shown, in the order the answer gives them: an image,
    /// or a page of a document drawn as one.
    pub pictures: Vec<fastctx_types::run_facts::StepPicture>,
}

/// Shows every requested path and records whether any could not be read.
pub fn view(paths: &[PathBuf], options: &Options, out: &mut impl Write) -> io::Result<Viewed> {
    if options.exploring() && paths.len() > 1 {
        return explore_several(paths, options, out);
    }
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
            let (section, observed) = match look(path, options, left + note.cost, explore::Limits::alone()) {
                Ok(result) => result,
                Err(error) => (Section::error(path, options, &describe_error(path, &error)), None),
            };
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
    let summary = (shown < paths.len())
        .then(|| format!("=== view ({files}: {shown} shown, {} not shown) ===", paths.len() - shown));
    write_answer(options, summary, sections, seen, out)
}

/// Several files looked at without a range: they share the one window a
/// file gets before its content is known (`world-view.md`, 探索). Each first
/// gets its sample, an equal share of the window; then the small ones are
/// shown whole, smallest first, while the window holds them. An image has no
/// part to sample: it is shown as itself, from the rest of the answer.
fn explore_several(paths: &[PathBuf], options: &Options, out: &mut impl Write) -> io::Result<Viewed> {
    let files = plural(paths.len() as u64, "file", "files");
    let summary_at_longest =
        format!("=== view ({files}: {n} shown whole, {n} in part, {n} not shown) ===\n", n = paths.len());
    let answer =
        bytes(if options.in_run { BUDGET - RUN_NOTES } else { BUDGET }).saturating_sub(summary_at_longest.len());
    let images: Vec<bool> = paths.iter().map(|path| shows_image(path)).collect();
    let sampled = images.iter().filter(|image| !**image).count();
    let window = if sampled > 0 { explore::WHOLE.min(answer) } else { 0 };
    let share = window / sampled.max(1);
    let mut pictures = answer - window;
    struct Planned {
        part: (Section, Option<Seen>),
        whole: Option<(Section, Option<Seen>)>,
    }
    let mut plans = Vec::new();
    for (path, &image) in paths.iter().zip(&images) {
        let note = not_shown(path, options);
        let looked = |budget, limits| {
            look(path, options, budget, limits)
                .unwrap_or_else(|error| (Section::error(path, options, &describe_error(path, &error)), None))
        };
        if image {
            let part = looked(pictures.max(note.cost), explore::Limits::alone());
            pictures = pictures.saturating_sub(part.0.cost);
            plans.push(Planned { part, whole: None });
            continue;
        }
        let small = fastctx_platform::local_path(path)
            .and_then(std::fs::metadata)
            .is_ok_and(|metadata| metadata.len() <= explore::WHOLE as u64);
        // A file that may fit whole is looked at whole first; a larger one
        // only as a sample, read once.
        let whole = if small {
            let seen = looked(window, explore::Limits::alone());
            seen.0.whole.then_some(seen)
        } else {
            None
        };
        let mut part = looked(share.max(note.cost), explore::Limits::share(share));
        if part.0.cost > share.max(note.cost) {
            // What cannot be sampled within its share is named.
            part = (note, None);
        }
        plans.push(Planned { part, whole });
    }
    let mut used: usize =
        plans.iter().zip(&images).filter(|(_, image)| !**image).map(|(plan, _)| plan.part.0.cost).sum();
    let mut order: Vec<usize> = (0..plans.len()).filter(|&i| plans[i].whole.is_some()).collect();
    order.sort_by_key(|&i| plans[i].whole.as_ref().map_or(usize::MAX, |whole| whole.0.cost));
    for i in order {
        let (part, whole) = (plans[i].part.0.cost, plans[i].whole.as_ref().map_or(usize::MAX, |whole| whole.0.cost));
        if used - part + whole <= window {
            used = used - part + whole;
            plans[i].part = plans[i].whole.take().expect("a whole file");
        }
    }
    let whole = plans.iter().filter(|plan| plan.part.0.whole).count();
    let not_shown = plans.iter().filter(|plan| !plan.part.0.shown).count();
    let summary = (whole < paths.len()).then(|| {
        format!(
            "=== view ({files}: {whole} shown whole, {} in part, {not_shown} not shown) ===",
            paths.len() - whole - not_shown
        )
    });
    let mut sections = Vec::new();
    let mut seen = Vec::new();
    for plan in plans {
        seen.extend(plan.part.1);
        sections.push(plan.part.0);
    }
    write_answer(options, summary, sections, seen, out)
}

/// Whether `view` shows the file as an image, which it has no part of to
/// sample.
fn shows_image(path: &Path) -> bool {
    let Ok(native) = fastctx_platform::local_path(path) else { return false };
    let Ok(file) = File::open(native) else { return false };
    matches!(crate::image::header(file), Ok(Some(Header::Shown { .. })))
}

fn write_answer(
    options: &Options,
    summary: Option<String>,
    sections: Vec<Section>,
    seen: Vec<Seen>,
    out: &mut impl Write,
) -> io::Result<Viewed> {
    let failed = sections.iter().any(|section| section.failed);
    let pictures = sections.iter().flat_map(|section| section.pictures.iter().cloned()).collect();
    if options.in_run {
        out.write_all(VIEW_MARK.as_bytes())?;
    }
    if let Some(summary) = summary {
        writeln!(out, "{summary}")?;
    }
    for section in sections {
        out.write_all(section.text.as_bytes())?;
    }
    if options.in_run {
        out.write_all(VIEW_END.as_bytes())?;
    }
    Ok(Viewed { seen, failed, pictures })
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
    failed: bool,
    /// Whether all of the file is shown.
    whole: bool,
    pictures: Vec<fastctx_types::run_facts::StepPicture>,
}

impl Section {
    fn note(path: &Path, options: &Options, facts: &str) -> Self {
        let text = format!("{}\n", head(path, options, facts));
        Self { cost: text.len(), text, shown: false, partial: false, failed: false, whole: false, pictures: Vec::new() }
    }

    fn error(path: &Path, options: &Options, facts: &str) -> Self {
        Self { failed: true, ..Self::note(path, options, facts) }
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
        Err(error) => return Section::error(path, options, &describe_error(path, &error)),
    };
    match std::fs::metadata(&native) {
        Ok(metadata) => Section::note(&native, options, &format!("not shown: {}", size(metadata.len()))),
        Err(error) => Section::error(&native, options, &describe_error(&native, &error)),
    }
}

fn look(path: &Path, options: &Options, budget: usize, limits: explore::Limits) -> io::Result<(Section, Option<Seen>)> {
    let native = match fastctx_platform::local_path(path) {
        Ok(path) => path,
        Err(error) => return Ok((Section::error(path, options, &describe_error(path, &error)), None)),
    };
    let resolved = match std::path::absolute(&native).and_then(|path| crate::files::canonical_path(&path)) {
        Ok(path) => path,
        Err(error) => return Ok((Section::error(&native, options, &describe_error(&native, &error)), None)),
    };
    let (section, mut seen) = look_file(&native, &resolved, options, budget, limits)?;
    if let Some(seen) = &mut seen {
        seen.path = resolved.to_string_lossy().into_owned();
    }
    Ok((section, seen))
}

fn look_file(
    path: &Path,
    resolved: &Path,
    options: &Options,
    budget: usize,
    limits: explore::Limits,
) -> io::Result<(Section, Option<Seen>)> {
    let metadata = match std::fs::metadata(resolved) {
        Ok(metadata) => metadata,
        Err(error) => return Ok((Section::error(path, options, &describe_error(path, &error)), None)),
    };
    if metadata.is_dir() {
        return Ok((Section::error(path, options, "a directory; list it with ls or find"), None));
    }
    if !metadata.is_file() {
        // A device, a FIFO or a socket could make looking wait forever.
        return Ok((Section::error(path, options, "a special file, not shown"), None));
    }
    let mut file = match File::open(resolved) {
        Ok(file) => file,
        Err(error) => return Ok((Section::error(path, options, &describe_error(path, &error)), None)),
    };
    let mut start = vec![0u8; 512];
    let read = read_up_to(&mut file, &mut start)?;
    start.truncate(read);
    // An image is told by walking its header from the start of the file, as
    // far as its size is: a phone's JPEG has tens of kilobytes before it.
    let detected_image = crate::image::header(&mut file)?;
    file.rewind()?;
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
        return Ok((Section::error(path, options, "--master applies to PowerPoint presentations"), None));
    }
    if options.sheet.is_some() || options.sheet_index.is_some() || options.range.is_some() {
        return Ok((Section::error(path, options, "--sheet and --range apply to Excel workbooks"), None));
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
            Section::error(path, options, "--pages selects PDF pages or PowerPoint slides; --image renders PDF pages"),
            None,
        ));
    }
    match detected_image {
        Some(Header::Shown { mime, width, height }) => {
            return image(path, options, budget, file, mime, width, height, metadata.len());
        }
        Some(Header::Other(format)) => {
            let facts = format!(
                "not shown: {} {format} image, {}; images are shown as PNG, JPEG, GIF or WebP",
                article(format),
                size(metadata.len())
            );
            return Ok((Section::note(path, options, &facts), None));
        }
        // An encoding asked for explicitly reads the file as text all the same.
        Some(Header::Unreadable { format, reason }) if options.encoding.is_none() => {
            let facts =
                format!("not shown: its first bytes are a {format} image's, but its header cannot be read: {reason}");
            return Ok((Section::error(path, options, &facts), None));
        }
        Some(Header::Unreadable { .. }) | None => {}
    }
    text::look(path, options, budget, file, start, &metadata, limits)
}

#[allow(clippy::too_many_arguments)]
fn image(
    path: &Path,
    options: &Options,
    budget: usize,
    mut file: File,
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
    let mut bytes = Vec::new();
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
    let picture = fastctx_types::run_facts::StepPicture {
        path: path.to_string_lossy().into_owned(),
        page: None,
        width,
        height,
        mime: mime.to_string(),
    };
    Ok((Section { text, cost, shown: true, partial: false, failed: false, whole: true, pictures: vec![picture] }, Some(seen)))
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

fn describe_error(path: &Path, error: &io::Error) -> String {
    if fastctx_platform::privacy::refused(&path.to_string_lossy(), error) {
        return fastctx_platform::privacy::REFUSAL.to_string();
    }
    match error.kind() {
        io::ErrorKind::NotFound => "no such file".to_string(),
        io::ErrorKind::PermissionDenied => format!("not readable: {error}"),
        _ => format!("could not be read: {error}"),
    }
}

/// The indefinite article for a format's name, as it is read aloud.
fn article(name: &str) -> &'static str {
    if name.starts_with(['A', 'E', 'I', 'O', 'U']) { "an" } else { "a" }
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
