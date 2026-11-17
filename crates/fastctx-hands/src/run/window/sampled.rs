//! Large captures decode bounded byte ranges through the ordinary terminal reader.

use std::fs::File;
use std::io::{self, Read as _, Seek as _, SeekFrom};
use std::path::Path;

use fastctx_types::run::{OutputPiece, OutputSample, OutputWindow};

/// Full terminal replay is reserved for captures within this fixed input budget.
pub(super) const REPLAY_BYTES: u64 = 8 * 1024 * 1024;
const HEAD_BYTES: u64 = 8_000;
const TAIL_BYTES: u64 = 4_000;
const GRAPHICS_PREFIX: u64 = 64 * 1024;
// Base64 and framing around the existing raw-image allowance are bounded too.
const GRAPHICS_BYTES: u64 = (super::IMAGE_BYTES as u64).div_ceil(3) * 4 + REPLAY_BYTES;

pub(super) fn replay(file: &mut File, size: u64) -> io::Result<bool> {
    if size <= REPLAY_BYTES {
        return Ok(true);
    }
    if size > GRAPHICS_BYTES {
        return Ok(false);
    }
    let prefix = range(file, 0, GRAPHICS_PREFIX)?;
    file.seek(SeekFrom::Start(0))?;
    Ok(memchr::memmem::find(&prefix, b"\x1b]1337;File=").is_some()
        || memchr::memmem::find(&prefix, b"\x1b_G").is_some())
}

pub(super) fn read(file: &mut File, log: &Path, size: u64) -> io::Result<OutputWindow> {
    let mut head = range(file, 0, HEAD_BYTES)?;
    let mut tail_from = size - TAIL_BYTES;
    let tail = range(file, tail_from, TAIL_BYTES)?;
    // Prefer complete raw rows at the cuts, so a partial multibyte character
    // does not change the code-page choice for an otherwise complete row.
    if let Some(end) = memchr::memrchr(b'\n', &head) {
        head.truncate(end + 1);
    }
    let tail = if let Some(end) = memchr::memchr(b'\n', &tail) {
        tail_from += (end + 1) as u64;
        &tail[end + 1..]
    } else {
        &tail[..]
    };
    let head_bytes = head.len() as u64;
    let mut window = decoded(&head, log)?;
    let end = decoded(tail, log)?;
    window.bytes = size;
    window.lines = 0;
    window.sampled = Some(OutputSample { head_bytes, tail_from });
    window.pieces.push(OutputPiece::Unreadable {
        line: 0,
        reason: format!(
            "raw bytes {head_bytes}-{tail_from} were not scanned; line counts, graphics and command failure markers were not fully read; terminal state at the cuts is unknown; complete output: {}",
            log.display()
        ),
    });
    window.pieces.extend(end.pieces);
    for legacy in end.legacy {
        if let Some(kept) = window.legacy.iter_mut().find(|kept| kept.encoding == legacy.encoding) {
            kept.lines += legacy.lines;
        } else {
            window.legacy.push(legacy);
        }
    }
    window.escaped_lines += end.escaped_lines;
    window.failed.extend(end.failed);
    Ok(window)
}

fn decoded(bytes: &[u8], log: &Path) -> io::Result<OutputWindow> {
    let mut lines = super::Lines::new(bytes);
    let mut assembly = super::Assembly::default();
    while let Some(line) = lines.next_line()? {
        assembly.whole(&line)?;
    }
    let mut window = assembly.finish(log, bytes.len() as u64, 0);
    window.failed = lines.failed().to_vec();
    for piece in &mut window.pieces {
        match piece {
            OutputPiece::Text { first_line, .. } | OutputPiece::Omitted { first_line, .. } => *first_line = 0,
            OutputPiece::PartOfLine { line, .. } | OutputPiece::Unreadable { line, .. } => *line = 0,
            OutputPiece::Image { .. } => {}
        }
    }
    Ok(window)
}

fn range(file: &mut File, from: u64, size: u64) -> io::Result<Vec<u8>> {
    file.seek(SeekFrom::Start(from))?;
    let mut bytes = vec![0; size as usize];
    file.read_exact(&mut bytes)?;
    Ok(bytes)
}
