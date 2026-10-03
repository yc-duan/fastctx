//! Ordered, bounded file viewing shared by the CLI and shell integrations.

mod animation;
mod args;
mod gif;
mod image;
#[cfg(feature = "pdf")]
mod pdf;
#[cfg(feature = "pdf")]
mod pdf_document;
#[cfg(test)]
mod tests;
mod text;

use crate::encoding::view::{OpenedText, TextReader};
use crate::head_note::{HeadMetric, HeadNote};
use crate::town::seen::{self, Seen};
pub use args::{Range, ViewArgs};
use image::Picture;
use std::fs::{self, File};
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

pub(super) const CHARACTER_BUDGET: usize = 150_000;
pub(super) const IMAGE_CHARACTERS: usize = 5_200;

#[derive(Clone)]
enum Block {
    Text(String),
    Image(Picture),
}

impl Block {
    fn cost(&self) -> usize {
        match self {
            Self::Text(text) => text.chars().count(),
            Self::Image(image) => image.cost(),
        }
    }
}

#[derive(Clone)]
struct Entry {
    path: PathBuf,
    subject: String,
    bytes: u64,
    fallback: String,
    error: bool,
    shown: bool,
    blocks: Vec<Block>,
    /// The file as it was when opened for showing, and whether all of its content was shown.
    opened: Option<Seen>,
    whole: bool,
}

/// What reading one file gave.
struct Reading {
    blocks: Vec<Block>,
    /// Whether the blocks reach the end of what was asked for; viewing stops at the first file
    /// that does not.
    complete: bool,
    /// Whether the blocks show the file's content, not a note about it.
    content: bool,
    encoding: Option<&'static str>,
}

impl Reading {
    fn of(blocks: Vec<Block>, complete: bool, content: bool) -> Self {
        Self {
            blocks,
            complete,
            content,
            encoding: None,
        }
    }
}

/// Executes view using the enrolled node name, or the operating system hostname. Run on
/// behalf of a resident of the World, it also records what the resident saw (`town::seen`).
pub fn run(args: &ViewArgs) -> Result<ExitCode, String> {
    let node = node_name()?;
    let (failed, shown) = view(args, &node, &mut std::io::stdout().lock())?;
    if let Err(error) = remember(shown) {
        eprintln!(
            "fastctx view: cannot record what was seen, so a later whole-file write cannot check whether these files changed since: {error}"
        );
    }
    Ok(if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

/// Writes standard OSC 1337 image sequences and UTF-8 text in argument order.
/// Returns true if any file failed. The caller supplies the local machine's name;
/// the paths are always opened in this process, never on a different node.
pub fn write(args: &ViewArgs, node: &str, output: &mut impl Write) -> Result<bool, String> {
    view(args, node, output).map(|(failed, _)| failed)
}

/// Appends what a resident saw to its `seen` record on this machine, when a resident is the
/// one viewing. A file whose content was shown whole is remembered by its hash, but only if
/// its size and modification time are still those it had when opened; otherwise, and for a
/// file shown in part, by that earlier size and time, which a later whole-file write compares.
fn remember(shown: Vec<(Seen, bool)>) -> Result<(), String> {
    let Some(resident) = std::env::var(crate::town::session::RESIDENT_VARIABLE)
        .ok()
        .filter(|id| !id.is_empty())
    else {
        return Ok(());
    };
    let paths =
        crate::world::WorldPaths::from_control(&crate::control::paths::ControlPaths::discover()?);
    let config = crate::world::load_config(&paths)?.ok_or("this machine is not in a World")?;
    let query = crate::town::who::Query {
        ids: vec![resident.clone()],
        ..Default::default()
    };
    let found = crate::town::who::answer(&paths.dir, &config.name, &query);
    // A resident of this machine is wherever its records are now; one living on another
    // machine has here only the directory of the town its `run` named.
    let dir = match crate::town::who::current(found.residents.iter().collect()) {
        Some(facts) => facts.dir.clone(),
        None => {
            let town = std::env::var(crate::town::cli::TOWN_VARIABLE)
                .ok()
                .filter(|town| !town.is_empty());
            crate::town::records::resident_dir(&paths.dir, town.as_deref(), &resident)
        }
    };
    let mut records = Vec::with_capacity(shown.len());
    for (opened, whole) in shown {
        if whole {
            let bytes = fs::read(&opened.path).map_err(|error| {
                format!(
                    "Cannot read {}: {error}",
                    crate::paths::display_path(&opened.path)
                )
            })?;
            let now = seen::observe(&opened.path, Some(&bytes), opened.encoding.as_deref())?;
            if (now.size, now.modified) == (opened.size, opened.modified) {
                records.push(now);
                continue;
            }
        }
        records.push(opened);
    }
    seen::record(&dir, &records)
}

/// Shows the files, and returns whether any failed and, for each file shown, the file as it
/// was when opened and whether all of its content was shown.
fn view(
    args: &ViewArgs,
    node: &str,
    output: &mut impl Write,
) -> Result<(bool, Vec<(Seen, bool)>), String> {
    let paths = args::paths(args)?;
    let mut entries: Vec<_> = paths.into_iter().map(|path| entry(path, node)).collect();
    let count = entries.len();
    // Reserve the longest possible decimal counts, and every remaining filename.
    // An arbitrary path list can itself exceed the budget; report that contradiction.
    let summary_reserve = summary(count, count, count).chars().count();
    let mut remaining = CHARACTER_BUDGET.checked_sub(summary_reserve + entries.iter().map(|entry| entry.fallback.chars().count()).sum::<usize>())
        .ok_or("Cannot list all requested filenames within the output budget. Split the path list into smaller calls.")?;
    let mut stopped = false;
    for index in 0..entries.len() {
        if entries[index].error || stopped {
            continue;
        }
        let available = remaining + entries[index].fallback.chars().count();
        let result = read(&entries[index], args, available);
        match result {
            Ok((
                Reading {
                    blocks,
                    complete,
                    content,
                    encoding,
                },
                opened,
            )) if !blocks.is_empty() => {
                let pictures: Vec<_> = entries
                    .iter()
                    .flat_map(|entry| &entry.blocks)
                    .chain(&blocks)
                    .filter_map(|block| match block {
                        Block::Image(image) => Some(image),
                        _ => None,
                    })
                    .collect();
                let resizing = pictures.iter().map(|image| image.bytes).sum::<u64>()
                    > image::byte_limit(pictures.len());
                // Resizing changes head-note length. Keep the preceding result so an
                // image whose final description does not fit can remain not shown.
                let before_resizing = resizing.then(|| entries.clone());
                entries[index].blocks = blocks;
                entries[index].shown = true;
                entries[index].opened = Some(Seen {
                    encoding: encoding.map(str::to_string),
                    ..opened
                });
                // A range of lines or pages is not the whole file, even when all of it fits.
                entries[index].whole =
                    complete && content && args.lines.is_none() && args.pages.is_none();
                shrink_images(&mut entries);
                let cost = entries
                    .iter()
                    .map(|entry| {
                        if entry.blocks.is_empty() {
                            entry.fallback.chars().count()
                        } else {
                            entry.blocks.iter().map(Block::cost).sum::<usize>()
                        }
                    })
                    .sum::<usize>();
                let Some(after) = CHARACTER_BUDGET.checked_sub(summary_reserve + cost) else {
                    if let Some(previous) = before_resizing
                        && entries
                            .iter()
                            .zip(&previous)
                            .all(|(current, prior)| !current.error || prior.error)
                    {
                        entries = previous;
                        stopped = true;
                        continue;
                    }
                    return Err("Image resize annotations exceed the output budget.".to_string());
                };
                remaining = after;
                stopped = !complete && !entries[index].error;
            }
            Ok(_) => stopped = true,
            Err(error) => {
                let report = note(&entries[index].subject, &error);
                if report.chars().count() > available {
                    return Err(error);
                }
                remaining = available - report.chars().count();
                entries[index].fallback = report;
                entries[index].error = true;
            }
        }
    }
    let shown = entries.iter().filter(|entry| entry.shown).count();
    output
        .write_all(summary(count, shown, count - shown).as_bytes())
        .map_err(|error| error.to_string())?;
    for entry in &mut entries {
        if entry.blocks.is_empty() {
            output
                .write_all(entry.fallback.as_bytes())
                .map_err(|error| error.to_string())?;
        } else {
            for block in &mut entry.blocks {
                match block {
                    Block::Text(text) => output.write_all(text.as_bytes()),
                    Block::Image(image) => image.write(output),
                }
                .map_err(|error| error.to_string())?;
            }
        }
    }
    output.flush().map_err(|error| error.to_string())?;
    let failed = entries.iter().any(|entry| entry.error);
    let shown = entries
        .into_iter()
        .filter(|entry| entry.shown)
        .filter_map(|entry| Some((entry.opened?, entry.whole)))
        .collect();
    Ok((failed, shown))
}

fn entry(path: PathBuf, node: &str) -> Entry {
    let absolute = match std::path::absolute(&path) {
        Ok(absolute) => absolute,
        Err(error) => {
            let subject = format!("{node}:{}", crate::path_codec::display_path(&path));
            return Entry {
                path,
                fallback: note(&subject, &format!("Cannot resolve path: {error}")),
                subject,
                bytes: 0,
                error: true,
                shown: false,
                blocks: Vec::new(),
                opened: None,
                whole: false,
            };
        }
    };
    let subject = format!("{node}:{}", crate::path_codec::display_path(&absolute));
    #[cfg(windows)]
    let device_path = absolute.as_os_str().to_string_lossy();
    #[cfg(windows)]
    let device_namespace = device_path.strip_prefix(r"\\.\");
    // The device namespace also contains ordinary disk files. Reject only pipe
    // endpoints before stat, without opening a client connection. (2026-10-02)
    #[cfg(windows)]
    if device_namespace.is_some_and(|path| {
        path.split(['\\', '/'])
            .next()
            .is_some_and(|name| name.eq_ignore_ascii_case("pipe"))
    }) {
        return Entry {
            path,
            subject: subject.clone(),
            bytes: 0,
            fallback: note(
                &subject,
                "Cannot read non-regular file. Only regular files are supported.",
            ),
            error: true,
            shown: false,
            blocks: Vec::new(),
            opened: None,
            whole: false,
        };
    }
    let (bytes, error) = match fs::metadata(&path) {
        #[cfg(windows)]
        Err(error)
            if device_namespace.is_some()
                && error.raw_os_error()
                    == Some(windows_sys::Win32::Foundation::ERROR_INVALID_FUNCTION as i32) =>
        {
            (
                0,
                Some("Cannot read non-regular file. Only regular files are supported.".to_string()),
            )
        }
        Ok(metadata) if metadata.is_dir() => (
            0,
            Some(
                "is a directory, not a file. Use ls, find, or rg --files to list its contents."
                    .to_string(),
            ),
        ),
        Ok(metadata) if !metadata.is_file() => (
            metadata.len(),
            Some("Cannot read non-regular file. Only regular files are supported.".to_string()),
        ),
        Ok(metadata) => (metadata.len(), None),
        Err(error) => (0, Some(io_error(&error))),
    };
    let fallback = note(
        &subject,
        error
            .as_deref()
            .unwrap_or(&format!("not shown: {}", size(bytes))),
    );
    Entry {
        path,
        subject,
        bytes,
        fallback,
        error: error.is_some(),
        shown: false,
        blocks: Vec::new(),
        opened: None,
        whole: false,
    }
}

/// Reads one file, with the file as it was when opened.
fn read(entry: &Entry, args: &ViewArgs, budget: usize) -> Result<(Reading, Seen), String> {
    let (file, metadata) = open_regular(&entry.path)?;
    // Recorded absolute, as the write tools name files.
    let absolute = std::path::absolute(&entry.path)
        .map_err(|error| format!("Cannot resolve path: {error}"))?;
    let opened = seen::measure(&absolute, &metadata, None);
    Ok((decode(entry, args, budget, file)?, opened))
}

fn decode(
    entry: &Entry,
    args: &ViewArgs,
    budget: usize,
    mut file: File,
) -> Result<Reading, String> {
    let mut prefix = [0_u8; 512];
    let length = file.read(&mut prefix).map_err(|error| io_error(&error))?;
    file.rewind().map_err(|error| io_error(&error))?;
    let prefix = &prefix[..length];
    if prefix.starts_with(b"%PDF-")
        || entry
            .path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("pdf"))
    {
        #[cfg(feature = "pdf")]
        return pdf::read(file, &entry.subject, entry.bytes, args, budget)
            .map(|(blocks, complete)| Reading::of(blocks, complete, true));
        #[cfg(not(feature = "pdf"))]
        return Err("PDF support is not enabled in this build.".to_string());
    }
    if ::image::guess_format(prefix).is_ok() {
        let image = Picture::open(&entry.subject, file)?;
        return Ok(if image.cost() <= budget {
            Reading::of(vec![Block::Image(image)], true, true)
        } else {
            Reading::of(Vec::new(), false, true)
        });
    }
    if let Some(kind) = crate::binary::detect_binary_type(prefix) {
        let output = note(&entry.subject, &format!("{kind}: {}", size(entry.bytes)));
        return Ok(if output.chars().count() <= budget {
            Reading::of(vec![Block::Text(output)], true, false)
        } else {
            Reading::of(Vec::new(), false, false)
        });
    }
    match TextReader::open(file, args.encoding.as_deref(), CHARACTER_BUDGET)? {
        OpenedText::Binary => {
            let output = note(
                &entry.subject,
                &format!("binary file: {}", size(entry.bytes)),
            );
            Ok(if output.chars().count() <= budget {
                Reading::of(vec![Block::Text(output)], true, false)
            } else {
                Reading::of(Vec::new(), false, false)
            })
        }
        OpenedText::Text(mut reader) => {
            let total = reader.total_lines;
            let window = text::TextWindow::read(|| reader.next_char(), args.lines, budget, total)?;
            let reading =
                match window.fit(&entry.subject, entry.bytes, reader.fact.as_deref(), budget) {
                    Some((text, complete)) => Reading::of(vec![Block::Text(text)], complete, true),
                    None => Reading::of(Vec::new(), false, true),
                };
            Ok(Reading {
                encoding: Some(reader.label()),
                ..reading
            })
        }
    }
}

fn open_regular(path: &Path) -> Result<(File, fs::Metadata), String> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // A file can become a FIFO after metadata inspection. Nonblocking open makes
        // that race rejectable without waiting for a writer. (2026-10-02)
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = options.open(path).map_err(|error| io_error(&error))?;
    let metadata = file.metadata().map_err(|error| io_error(&error))?;
    if !metadata.is_file() {
        return Err("Cannot read non-regular file. Only regular files are supported.".to_string());
    }
    Ok((file, metadata))
}

fn io_error(error: &std::io::Error) -> String {
    match error.kind() {
        std::io::ErrorKind::NotFound => "File does not exist.".to_string(),
        std::io::ErrorKind::PermissionDenied => "Permission denied.".to_string(),
        _ => format!("Cannot open file: {error}"),
    }
}

pub(super) fn note(subject: &str, metric: &str) -> String {
    format!(
        "{}\n",
        HeadNote::new(subject, HeadMetric::event(metric)).render()
    )
}

fn summary(files: usize, shown: usize, omitted: usize) -> String {
    note(
        "view",
        &format!("{files} files: {shown} shown, {omitted} not shown"),
    )
}

fn size(bytes: u64) -> String {
    if bytes < 1_000 {
        format!("{bytes} bytes")
    } else if bytes < 1_000_000 {
        format!("{:.1} KB", bytes as f64 / 1_000.0)
    } else if bytes < 1_000_000_000 {
        format!("{:.1} MB", bytes as f64 / 1_000_000.0)
    } else {
        format!("{:.1} GB", bytes as f64 / 1_000_000_000.0)
    }
}

fn shrink_images(entries: &mut [Entry]) {
    loop {
        let indexed: Vec<_> = entries
            .iter_mut()
            .enumerate()
            .flat_map(|(index, entry)| entry.blocks.iter_mut().map(move |block| (index, block)))
            .filter_map(|(index, block)| match block {
                Block::Image(image) => Some((index, image)),
                _ => None,
            })
            .collect();
        let (indices, mut images): (Vec<_>, Vec<_>) = indexed.into_iter().unzip();
        let Err((index, error)) = image::shrink(&mut images) else {
            return;
        };
        drop(images);
        let entry = &mut entries[indices[index]];
        entry.blocks.clear();
        entry.shown = false;
        entry.error = true;
        entry.fallback = note(&entry.subject, &error);
    }
}

fn node_name() -> Result<String, String> {
    let control = crate::control::paths::ControlPaths::discover()?;
    let paths = crate::world::WorldPaths::from_control(&control);
    if let Some(config) = crate::world::load_config(&paths)? {
        return Ok(config.name);
    }
    let output = std::process::Command::new("hostname")
        .output()
        .map_err(|error| format!("Cannot determine machine name: {error}"))?;
    if !output.status.success() {
        return Err("Cannot determine machine name: hostname failed.".to_string());
    }
    let hostname = String::from_utf8(output.stdout)
        .map_err(|error| format!("Cannot decode machine name: {error}"))?;
    Ok(hostname.trim().to_string())
}
