//! Append-only, buffered, rotating disk window for normalized background output.

use super::model::SpoolLine;
use super::store::segment_paths;
use crate::paths::display_path;
use crate::shell::buffer::OUTPUT_BUFFER_BYTES;
use crate::shell::normalize::NormalizedLine;
use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const SEGMENT_TARGET_BYTES: u64 = 1024 * 1024;
const FLUSH_BYTES: usize = 64 * 1024;
const FLUSH_IDLE: Duration = Duration::from_millis(250);

#[derive(Debug)]
struct OpenSegment {
    path: PathBuf,
    writer: BufWriter<File>,
    logical_bytes: u64,
}

/// The supervisor's sole append writer; readers only ever observe complete newline records.
#[derive(Debug)]
pub(crate) struct SpoolWriter {
    directory: PathBuf,
    segment: Option<OpenSegment>,
    next_seq: u64,
    had_loss: bool,
    last_flush: Instant,
}

impl SpoolWriter {
    pub(crate) fn new(directory: &Path) -> Self {
        Self {
            directory: directory.to_path_buf(),
            segment: None,
            next_seq: 1,
            had_loss: false,
            last_flush: Instant::now(),
        }
    }

    pub(crate) fn append(&mut self, line: NormalizedLine) -> Result<u64, String> {
        let seq = self.next_seq;
        if self
            .segment
            .as_ref()
            .is_some_and(|segment| segment.logical_bytes >= SEGMENT_TARGET_BYTES)
        {
            self.rotate()?;
        }
        if self.segment.is_none() {
            self.segment = Some(open_segment(&self.directory, seq)?);
        }
        let record = SpoolLine {
            seq,
            text: line.text,
            truncated: line.truncated,
            had_loss: self.had_loss || line.truncated,
        };
        let mut bytes = serde_json::to_vec(&record)
            .map_err(|error| format!("Cannot encode background output at seq {seq}: {error}"))?;
        bytes.push(b'\n');
        let segment = self.segment.as_mut().expect("segment was opened above");
        segment.writer.write_all(&bytes).map_err(|error| {
            format!(
                "cannot append output segment {}: {error}",
                display_path(&segment.path)
            )
        })?;
        segment.logical_bytes = segment.logical_bytes.saturating_add(bytes.len() as u64);
        self.next_seq = self.next_seq.saturating_add(1);
        self.had_loss |= line.truncated;
        if segment.writer.buffer().len() >= FLUSH_BYTES {
            self.flush()?;
        }
        self.enforce_window()?;
        Ok(seq)
    }

    pub(crate) fn flush_if_idle(&mut self) -> Result<(), String> {
        if self
            .segment
            .as_ref()
            .is_some_and(|segment| !segment.writer.buffer().is_empty())
            && self.last_flush.elapsed() >= FLUSH_IDLE
        {
            self.flush()?;
        }
        Ok(())
    }

    pub(crate) fn finish(&mut self) -> Result<(), String> {
        self.flush()
    }

    pub(crate) fn total_lines(&self) -> u64 {
        self.next_seq.saturating_sub(1)
    }

    pub(crate) fn had_loss(&self) -> bool {
        self.had_loss
    }

    fn rotate(&mut self) -> Result<(), String> {
        self.flush()?;
        self.segment = None;
        Ok(())
    }

    fn flush(&mut self) -> Result<(), String> {
        if let Some(segment) = self.segment.as_mut() {
            segment.writer.flush().map_err(|error| {
                format!(
                    "cannot flush output segment {}: {error}",
                    display_path(&segment.path)
                )
            })?;
        }
        self.last_flush = Instant::now();
        Ok(())
    }

    fn enforce_window(&mut self) -> Result<(), String> {
        let mut paths = segment_paths(&self.directory)?;
        paths.sort();
        let current = self.segment.as_ref().map(|segment| segment.path.as_path());
        let buffered = self
            .segment
            .as_ref()
            .map_or(0, |segment| segment.writer.buffer().len() as u64);
        let mut total = buffered;
        let mut sized = Vec::with_capacity(paths.len());
        for path in paths {
            let size = fs::metadata(&path)
                .map_err(|error| {
                    format!(
                        "cannot inspect output segment {}: {error}",
                        display_path(&path)
                    )
                })?
                .len();
            total = total.saturating_add(size);
            sized.push((path, size));
        }
        for (path, size) in sized {
            if total <= OUTPUT_BUFFER_BYTES as u64 {
                break;
            }
            if current == Some(path.as_path()) {
                continue;
            }
            match fs::remove_file(&path) {
                Ok(()) => {
                    total = total.saturating_sub(size);
                    self.had_loss = true;
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    total = total.saturating_sub(size);
                    self.had_loss = true;
                }
                Err(error) => {
                    return Err(format!(
                        "cannot rotate output segment {}: {error}",
                        display_path(&path)
                    ));
                }
            }
        }
        Ok(())
    }
}

fn open_segment(directory: &Path, first_seq: u64) -> Result<OpenSegment, String> {
    let path = directory.join(format!("segment-{first_seq:020}.jsonl"));
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(&path).map_err(|error| {
        format!(
            "cannot create output segment {}: {error}",
            display_path(&path)
        )
    })?;
    Ok(OpenSegment {
        path,
        writer: BufWriter::with_capacity(FLUSH_BYTES, file),
        logical_bytes: 0,
    })
}

#[cfg(test)]
mod tests {
    use super::{SpoolWriter, segment_paths};
    use crate::shell::buffer::OUTPUT_BUFFER_BYTES;
    use crate::shell::normalize::NormalizedLine;

    #[test]
    fn disk_window_drops_old_segments_without_stopping_or_miscounting_output() {
        let temp = tempfile::tempdir().unwrap();
        let mut spool = SpoolWriter::new(temp.path());
        let payload = "x".repeat(1_990);
        for index in 0..5_000 {
            spool
                .append(NormalizedLine {
                    text: format!("{index:04}-{payload}"),
                    terminated: true,
                    truncated: false,
                })
                .unwrap();
        }
        spool.finish().unwrap();

        let segments = segment_paths(temp.path()).unwrap();
        let retained_bytes = segments
            .iter()
            .map(|path| std::fs::metadata(path).unwrap().len())
            .sum::<u64>();
        assert_eq!(spool.total_lines(), 5_000);
        assert!(spool.had_loss());
        assert!(retained_bytes <= OUTPUT_BUFFER_BYTES as u64);
        assert!(
            segments.iter().all(|path| {
                path.file_name().unwrap().to_string_lossy().as_ref()
                    != "segment-00000000000000000001.jsonl"
            }),
            "the oldest segment must be evicted from the rolling window"
        );
    }
}
