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
}

/// Executes view using the enrolled node name, or the operating system hostname.
pub fn run(args: &ViewArgs) -> Result<ExitCode, String> {
    let node = node_name()?;
    let failed = write(args, &node, &mut std::io::stdout().lock())?;
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
            Ok((blocks, complete)) if !blocks.is_empty() => {
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
    Ok(entries.iter().any(|entry| entry.error))
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
    }
}

fn read(entry: &Entry, args: &ViewArgs, budget: usize) -> Result<(Vec<Block>, bool), String> {
    let mut file = open_regular(&entry.path)?;
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
        return pdf::read(file, &entry.subject, entry.bytes, args, budget);
        #[cfg(not(feature = "pdf"))]
        return Err("PDF support is not enabled in this build.".to_string());
    }
    if ::image::guess_format(prefix).is_ok() {
        let image = Picture::open(&entry.subject, file)?;
        return Ok(if image.cost() <= budget {
            (vec![Block::Image(image)], true)
        } else {
            (Vec::new(), false)
        });
    }
    if let Some(kind) = crate::binary::detect_binary_type(prefix) {
        let output = note(&entry.subject, &format!("{kind}: {}", size(entry.bytes)));
        return Ok(if output.chars().count() <= budget {
            (vec![Block::Text(output)], true)
        } else {
            (Vec::new(), false)
        });
    }
    match TextReader::open(file, args.encoding.as_deref(), CHARACTER_BUDGET)? {
        OpenedText::Binary => {
            let output = note(
                &entry.subject,
                &format!("binary file: {}", size(entry.bytes)),
            );
            Ok(if output.chars().count() <= budget {
                (vec![Block::Text(output)], true)
            } else {
                (Vec::new(), false)
            })
        }
        OpenedText::Text(mut reader) => {
            let total = reader.total_lines;
            let window = text::TextWindow::read(|| reader.next_char(), args.lines, budget, total)?;
            Ok(
                match window.fit(&entry.subject, entry.bytes, reader.fact.as_deref(), budget) {
                    Some((text, complete)) => (vec![Block::Text(text)], complete),
                    None => (Vec::new(), false),
                },
            )
        }
    }
}

fn open_regular(path: &Path) -> Result<File, String> {
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
    if !file.metadata().map_err(|error| io_error(&error))?.is_file() {
        return Err("Cannot read non-regular file. Only regular files are supported.".to_string());
    }
    Ok(file)
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
