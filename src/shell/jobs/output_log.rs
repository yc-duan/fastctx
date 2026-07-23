//! Append-only plain-text background logs and their derived line-offset index.

use super::model::{OUTPUT_INDEX_FILE, OUTPUT_LOG_FILE, StoredLine};
use crate::paths::display_path;
use crate::shell::normalize::{NormalizedEvent, StreamEncoding};
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub(super) const INDEX_HEADER: &[u8; 8] = b"FCTXIDX1";
pub(super) const INDEX_ENTRY_BYTES: u64 = 24;
const FLUSH_BYTES: u64 = 64 * 1024;
const FLUSH_IDLE: Duration = Duration::from_millis(250);
const MAX_STORED_LINE_BYTES: u64 = 64 * 1024;
const MAX_RECOVERY_SCAN_BYTES: u64 = 256 * 1024;

/// One fixed-width derived index entry. The log remains the source of truth;
/// this sidecar only makes line-number lookup independent of log length.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct LineIndexEntry {
    pub(super) start: u64,
    pub(super) content_end: u64,
    pub(super) record_end: u64,
}

impl LineIndexEntry {
    fn encode(self, output: &mut Vec<u8>) {
        output.extend_from_slice(&self.start.to_le_bytes());
        output.extend_from_slice(&self.content_end.to_le_bytes());
        output.extend_from_slice(&self.record_end.to_le_bytes());
    }

    pub(super) fn decode(bytes: &[u8]) -> Option<Self> {
        let bytes: &[u8; 24] = bytes.try_into().ok()?;
        let start = u64::from_le_bytes(bytes[0..8].try_into().ok()?);
        let content_end = u64::from_le_bytes(bytes[8..16].try_into().ok()?);
        let record_end = u64::from_le_bytes(bytes[16..24].try_into().ok()?);
        (start <= content_end && content_end <= record_end).then_some(Self {
            start,
            content_end,
            record_end,
        })
    }
}

/// The detached supervisor's sole writer for a schema-v3 job. Both files are
/// append-only; the index is flushed only after the corresponding log bytes.
#[derive(Debug)]
pub(crate) struct OutputLogWriter {
    log_path: PathBuf,
    index_path: PathBuf,
    log: BufWriter<File>,
    index: File,
    pending_index: Vec<u8>,
    position: u64,
    current_start: u64,
    current_has_content: bool,
    total_lines: u64,
    durable_lines: u64,
    committed_position: u64,
    indexed_position: u64,
    stream_encoding: Option<StreamEncoding>,
    started: bool,
    bytes_since_flush: u64,
    last_flush: Instant,
}

impl OutputLogWriter {
    pub(crate) fn new(directory: &Path) -> Result<Self, String> {
        let log_path = directory.join(OUTPUT_LOG_FILE);
        let index_path = directory.join(OUTPUT_INDEX_FILE);
        let log = create_private_file(&log_path, "output log")?;
        let mut index = create_private_file(&index_path, "output index")?;
        if let Err(error) = index.write_all(INDEX_HEADER).and_then(|()| index.flush()) {
            let _ = std::fs::remove_file(&log_path);
            let _ = std::fs::remove_file(&index_path);
            return Err(format!(
                "cannot initialize background output index {}: {error}",
                display_path(&index_path)
            ));
        }
        Ok(Self {
            log_path,
            index_path,
            log: BufWriter::with_capacity(FLUSH_BYTES as usize, log),
            index,
            pending_index: Vec::with_capacity(FLUSH_BYTES as usize),
            position: 0,
            current_start: 0,
            current_has_content: false,
            total_lines: 0,
            durable_lines: 0,
            committed_position: 0,
            indexed_position: 0,
            stream_encoding: None,
            started: false,
            bytes_since_flush: 0,
            last_flush: Instant::now(),
        })
    }

    /// Appends one normalized stream event and returns the committed line number,
    /// if this event ended a line.
    pub(crate) fn append(&mut self, event: NormalizedEvent) -> Result<Option<u64>, String> {
        let committed = match event {
            NormalizedEvent::Start(encoding) => {
                if self.started {
                    return Err(
                        "cannot restart an initialized background output stream".to_string()
                    );
                }
                self.started = true;
                self.stream_encoding = encoding;
                if let Some(encoding) = encoding {
                    self.write_log(stream_bom(encoding))?;
                }
                self.current_start = self.position;
                None
            }
            NormalizedEvent::Bytes(bytes) => {
                self.require_started()?;
                if !bytes.is_empty() {
                    self.write_log(&bytes)?;
                    self.current_has_content = true;
                }
                None
            }
            NormalizedEvent::LineEnd { terminated } => {
                self.require_started()?;
                Some(self.commit_line(terminated)?)
            }
        };
        if self.bytes_since_flush >= FLUSH_BYTES
            || self
                .committed_position
                .saturating_sub(self.indexed_position)
                >= MAX_RECOVERY_SCAN_BYTES
        {
            self.flush()?;
        }
        Ok(committed)
    }

    pub(crate) fn flush_if_idle(&mut self) -> Result<(), String> {
        if self.has_buffered_data() && self.last_flush.elapsed() >= FLUSH_IDLE {
            self.flush()?;
        }
        Ok(())
    }

    /// Seals bytes already received as an unterminated final line, then flushes.
    /// This is also used when capture fails after some bytes have reached disk.
    pub(crate) fn finish(&mut self) -> Result<(), String> {
        if self.current_has_content {
            self.commit_line(false)?;
        }
        self.flush()
    }

    pub(crate) const fn total_lines(&self) -> u64 {
        self.total_lines
    }

    pub(crate) fn preserved_lines(&self) -> u64 {
        let log_len = self
            .log
            .get_ref()
            .metadata()
            .map(|metadata| metadata.len())
            .unwrap_or(self.indexed_position);
        self.durable_lines.saturating_add(
            self.pending_index
                .chunks_exact(INDEX_ENTRY_BYTES as usize)
                .filter_map(LineIndexEntry::decode)
                .take_while(|entry| entry.record_end <= log_len)
                .count() as u64,
        )
    }

    fn require_started(&self) -> Result<(), String> {
        if self.started {
            Ok(())
        } else {
            Err("background output arrived before its stream encoding was established".to_string())
        }
    }

    fn commit_line(&mut self, terminated: bool) -> Result<u64, String> {
        let content_end = self.position;
        if terminated {
            self.write_log(line_ending(self.stream_encoding))?;
        }
        LineIndexEntry {
            start: self.current_start,
            content_end,
            record_end: self.position,
        }
        .encode(&mut self.pending_index);
        self.total_lines = self.total_lines.saturating_add(1);
        self.committed_position = self.position;
        self.current_start = self.position;
        self.current_has_content = false;
        Ok(self.total_lines)
    }

    fn write_log(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.log.write_all(bytes).map_err(|error| {
            format!(
                "cannot append background output log {}: {error}",
                display_path(&self.log_path)
            )
        })?;
        self.position = self.position.saturating_add(bytes.len() as u64);
        self.bytes_since_flush = self.bytes_since_flush.saturating_add(bytes.len() as u64);
        Ok(())
    }

    fn has_buffered_data(&self) -> bool {
        !self.log.buffer().is_empty() || !self.pending_index.is_empty()
    }

    fn flush(&mut self) -> Result<(), String> {
        self.log.flush().map_err(|error| {
            format!(
                "cannot flush background output log {}: {error}",
                display_path(&self.log_path)
            )
        })?;
        if !self.pending_index.is_empty() {
            self.index.write_all(&self.pending_index).map_err(|error| {
                format!(
                    "cannot append background output index {}: {error}",
                    display_path(&self.index_path)
                )
            })?;
            self.index.flush().map_err(|error| {
                format!(
                    "cannot flush background output index {}: {error}",
                    display_path(&self.index_path)
                )
            })?;
            self.pending_index.clear();
            self.indexed_position = self.committed_position;
            self.durable_lines = self.total_lines;
        }
        self.bytes_since_flush = 0;
        self.last_flush = Instant::now();
        Ok(())
    }
}

/// A point-in-time reader over a schema-v3 log. The fixed-width sidecar makes
/// line lookup O(requested lines), independent of the full log length.
#[derive(Debug)]
pub(super) struct OutputLogReader {
    log_path: PathBuf,
    log: File,
    index: File,
    log_len: u64,
    bom_len: u64,
    indexed_lines: u64,
    tail_entries: Vec<LineIndexEntry>,
    stream_encoding: Option<StreamEncoding>,
}

#[derive(Debug)]
pub(super) struct BoundedLines {
    pub(super) lines: Vec<StoredLine>,
    pub(super) complete: bool,
}

impl OutputLogReader {
    /// Opens one stable snapshot. Unindexed bytes are ignored for a live stream,
    /// but become recoverable final lines once the job/capture is terminal.
    pub(super) fn open(directory: &Path, include_unindexed_tail: bool) -> Result<Self, String> {
        let log_path = directory.join(OUTPUT_LOG_FILE);
        let index_path = directory.join(OUTPUT_INDEX_FILE);
        let mut log = File::open(&log_path).map_err(|error| {
            format!(
                "Cannot read background output log {}: {error}",
                display_path(&log_path)
            )
        })?;
        let log_len = log
            .metadata()
            .map_err(|error| {
                format!(
                    "Cannot inspect background output log {}: {error}",
                    display_path(&log_path)
                )
            })?
            .len();
        let (stream_encoding, bom_len) = detect_log_encoding(&mut log, log_len)?;

        let mut index = File::open(&index_path).map_err(|error| {
            format!(
                "Cannot read background output index {}: {error}. The full log remains readable at {}.",
                display_path(&index_path),
                display_path(&log_path)
            )
        })?;
        let index_len = index
            .metadata()
            .map_err(|error| {
                format!(
                    "Cannot inspect background output index {}: {error}",
                    display_path(&index_path)
                )
            })?
            .len();
        if index_len < INDEX_HEADER.len() as u64 {
            return Err(damaged_index(&index_path, &log_path));
        }
        let mut header = [0_u8; INDEX_HEADER.len()];
        index
            .read_exact(&mut header)
            .map_err(|_| damaged_index(&index_path, &log_path))?;
        if &header != INDEX_HEADER {
            return Err(damaged_index(&index_path, &log_path));
        }
        let indexed_lines = index_len.saturating_sub(INDEX_HEADER.len() as u64) / INDEX_ENTRY_BYTES;
        let indexed_end = if indexed_lines == 0 {
            bom_len
        } else {
            read_validated_index_entry(
                &mut index,
                indexed_lines,
                indexed_lines,
                bom_len,
                log_len,
                &index_path,
                &log_path,
            )?
            .record_end
        };
        if indexed_end < bom_len || indexed_end > log_len {
            return Err(damaged_index(&index_path, &log_path));
        }
        let tail_entries = if include_unindexed_tail && indexed_end < log_len {
            recover_tail_entries(&mut log, indexed_end, log_len, stream_encoding, &log_path)?
        } else {
            Vec::new()
        };
        Ok(Self {
            log_path,
            log,
            index,
            log_len,
            bom_len,
            indexed_lines,
            tail_entries,
            stream_encoding,
        })
    }

    pub(super) fn path(&self) -> &Path {
        &self.log_path
    }

    pub(super) fn total_lines(&self) -> u64 {
        self.indexed_lines
            .saturating_add(self.tail_entries.len() as u64)
    }

    pub(super) fn read_range(&mut self, first: u64, last: u64) -> Result<Vec<StoredLine>, String> {
        if first == 0 || first > last {
            return Ok(Vec::new());
        }
        let last = last.min(self.total_lines());
        let mut lines = Vec::with_capacity(
            usize::try_from(last.saturating_sub(first).saturating_add(1)).unwrap_or(0),
        );
        for seq in first..=last {
            let entry = self.entry(seq)?;
            lines.push(read_stored_line(
                &mut self.log,
                seq,
                entry,
                self.stream_encoding,
                &self.log_path,
            )?);
        }
        Ok(lines)
    }

    pub(super) fn read_prefix_bounded(
        &mut self,
        first: u64,
        last: u64,
        max_lines: usize,
        max_bytes: usize,
    ) -> Result<BoundedLines, String> {
        if first == 0 || first > last || max_lines == 0 {
            return Ok(BoundedLines {
                lines: Vec::new(),
                complete: first > last,
            });
        }
        let last = last.min(self.total_lines());
        let mut lines = Vec::new();
        let mut stored_bytes = 0_usize;
        let mut next = first;
        while next <= last && lines.len() < max_lines {
            let line = self.read_one(next)?;
            let would_exceed =
                !lines.is_empty() && stored_bytes.saturating_add(line.bytes.len()) > max_bytes;
            if would_exceed {
                break;
            }
            stored_bytes = stored_bytes.saturating_add(line.bytes.len());
            lines.push(line);
            next = next.saturating_add(1);
        }
        Ok(BoundedLines {
            lines,
            complete: next > last,
        })
    }

    pub(super) fn read_suffix_bounded(
        &mut self,
        first: u64,
        last: u64,
        max_lines: usize,
        max_bytes: usize,
    ) -> Result<BoundedLines, String> {
        if first == 0 || first > last || max_lines == 0 {
            return Ok(BoundedLines {
                lines: Vec::new(),
                complete: first > last,
            });
        }
        let last = last.min(self.total_lines());
        let mut lines = Vec::new();
        let mut stored_bytes = 0_usize;
        let mut next = last;
        loop {
            let line = self.read_one(next)?;
            let would_exceed =
                !lines.is_empty() && stored_bytes.saturating_add(line.bytes.len()) > max_bytes;
            if would_exceed {
                break;
            }
            stored_bytes = stored_bytes.saturating_add(line.bytes.len());
            lines.push(line);
            if next == first || lines.len() >= max_lines {
                break;
            }
            next -= 1;
        }
        let complete = lines.last().is_some_and(|line| line.seq == first);
        lines.reverse();
        Ok(BoundedLines { lines, complete })
    }

    pub(super) fn record_end(&mut self, seq: u64) -> Result<u64, String> {
        if seq == 0 {
            return Ok(0);
        }
        Ok(self.entry(seq)?.record_end)
    }

    pub(super) const fn log_len(&self) -> u64 {
        self.log_len
    }

    fn entry(&mut self, seq: u64) -> Result<LineIndexEntry, String> {
        if seq <= self.indexed_lines {
            let index_path = self.log_path.with_file_name(OUTPUT_INDEX_FILE);
            return read_validated_index_entry(
                &mut self.index,
                seq,
                self.indexed_lines,
                self.bom_len,
                self.log_len,
                &index_path,
                &self.log_path,
            );
        }
        let tail_index = usize::try_from(seq.saturating_sub(self.indexed_lines + 1))
            .map_err(|_| "background output line number is too large".to_string())?;
        self.tail_entries.get(tail_index).copied().ok_or_else(|| {
            format!(
                "Cannot read line {seq} from background output log {}: only {} lines are available.",
                display_path(&self.log_path),
                self.total_lines()
            )
        })
    }

    fn read_one(&mut self, seq: u64) -> Result<StoredLine, String> {
        let entry = self.entry(seq)?;
        read_stored_line(
            &mut self.log,
            seq,
            entry,
            self.stream_encoding,
            &self.log_path,
        )
    }
}

fn read_index_entry(
    index: &mut File,
    seq: u64,
    index_path: &Path,
    log_path: &Path,
) -> Result<LineIndexEntry, String> {
    let offset = (INDEX_HEADER.len() as u64)
        .checked_add(seq.saturating_sub(1).saturating_mul(INDEX_ENTRY_BYTES))
        .ok_or_else(|| damaged_index(index_path, log_path))?;
    index
        .seek(SeekFrom::Start(offset))
        .and_then(|_| {
            let mut bytes = [0_u8; INDEX_ENTRY_BYTES as usize];
            index.read_exact(&mut bytes)?;
            Ok(bytes)
        })
        .map_err(|_| damaged_index(index_path, log_path))
        .and_then(|bytes| {
            LineIndexEntry::decode(&bytes).ok_or_else(|| damaged_index(index_path, log_path))
        })
}

fn read_validated_index_entry(
    index: &mut File,
    seq: u64,
    indexed_lines: u64,
    bom_len: u64,
    log_len: u64,
    index_path: &Path,
    log_path: &Path,
) -> Result<LineIndexEntry, String> {
    let entry = read_index_entry(index, seq, index_path, log_path)?;
    let begins_at_expected_offset = if seq == 1 {
        entry.start == bom_len
    } else {
        read_index_entry(index, seq - 1, index_path, log_path)?.record_end == entry.start
    };
    let ends_at_expected_offset = if seq == indexed_lines {
        entry.record_end <= log_len
    } else {
        entry.record_end == read_index_entry(index, seq + 1, index_path, log_path)?.start
    };
    if !begins_at_expected_offset || !ends_at_expected_offset || entry.record_end > log_len {
        return Err(damaged_index(index_path, log_path));
    }
    Ok(entry)
}

fn read_stored_line(
    log: &mut File,
    seq: u64,
    entry: LineIndexEntry,
    stream_encoding: Option<StreamEncoding>,
    path: &Path,
) -> Result<StoredLine, String> {
    let total_bytes = entry.content_end.saturating_sub(entry.start);
    let shown_bytes = total_bytes.min(MAX_STORED_LINE_BYTES);
    let length = usize::try_from(shown_bytes).map_err(|_| {
        format!(
            "Cannot read line {seq} from background output log {}: the line is too large to address.",
            display_path(path)
        )
    })?;
    let mut bytes = vec![0_u8; length];
    log.seek(SeekFrom::Start(entry.start))
        .and_then(|_| log.read_exact(&mut bytes))
        .map_err(|error| {
            format!(
                "Cannot read line {seq} from background output log {}: {error}",
                display_path(path)
            )
        })?;
    Ok(StoredLine {
        seq,
        bytes,
        total_bytes,
        stream_encoding,
        legacy_text: None,
        known_truncated: shown_bytes < total_bytes,
    })
}

fn detect_log_encoding(
    log: &mut File,
    log_len: u64,
) -> Result<(Option<StreamEncoding>, u64), String> {
    let mut prefix = [0_u8; 4];
    let length = usize::try_from(log_len.min(4)).unwrap_or(4);
    log.seek(SeekFrom::Start(0))
        .and_then(|_| log.read_exact(&mut prefix[..length]))
        .map_err(|error| format!("Cannot inspect the background output log encoding: {error}"))?;
    let bytes = &prefix[..length];
    let detected = if bytes.starts_with(stream_bom(StreamEncoding::Utf32Be)) {
        (Some(StreamEncoding::Utf32Be), 4)
    } else if bytes.starts_with(stream_bom(StreamEncoding::Utf32Le)) {
        (Some(StreamEncoding::Utf32Le), 4)
    } else if bytes.starts_with(stream_bom(StreamEncoding::Utf16Be)) {
        (Some(StreamEncoding::Utf16Be), 2)
    } else if bytes.starts_with(stream_bom(StreamEncoding::Utf16Le)) {
        (Some(StreamEncoding::Utf16Le), 2)
    } else {
        (None, 0)
    };
    Ok(detected)
}

fn recover_tail_entries(
    log: &mut File,
    start: u64,
    end: u64,
    stream_encoding: Option<StreamEncoding>,
    path: &Path,
) -> Result<Vec<LineIndexEntry>, String> {
    let length = end.saturating_sub(start);
    if length > MAX_RECOVERY_SCAN_BYTES {
        let ending = line_ending(stream_encoding);
        let terminated = file_ends_with(log, ending, end, path)?;
        return Ok(vec![LineIndexEntry {
            start,
            content_end: end.saturating_sub(if terminated { ending.len() as u64 } else { 0 }),
            record_end: end,
        }]);
    }
    let mut bytes = vec![0_u8; usize::try_from(length).unwrap_or(0)];
    log.seek(SeekFrom::Start(start))
        .and_then(|_| log.read_exact(&mut bytes))
        .map_err(|error| {
            format!(
                "Cannot recover the final background output bytes from {}: {error}",
                display_path(path)
            )
        })?;
    let ending = line_ending(stream_encoding);
    let width = ending.len();
    let mut entries = Vec::new();
    let mut line_start = 0_usize;
    let mut cursor = 0_usize;
    while cursor.saturating_add(width) <= bytes.len() {
        if &bytes[cursor..cursor + width] == ending {
            entries.push(LineIndexEntry {
                start: start + line_start as u64,
                content_end: start + cursor as u64,
                record_end: start + (cursor + width) as u64,
            });
            cursor += width;
            line_start = cursor;
        } else {
            cursor += width;
        }
    }
    if line_start < bytes.len() {
        entries.push(LineIndexEntry {
            start: start + line_start as u64,
            content_end: end,
            record_end: end,
        });
    }
    Ok(entries)
}

fn file_ends_with(log: &mut File, suffix: &[u8], end: u64, path: &Path) -> Result<bool, String> {
    if end < suffix.len() as u64 {
        return Ok(false);
    }
    let mut actual = vec![0_u8; suffix.len()];
    log.seek(SeekFrom::Start(end - suffix.len() as u64))
        .and_then(|_| log.read_exact(&mut actual))
        .map_err(|error| {
            format!(
                "Cannot inspect the final background output bytes in {}: {error}",
                display_path(path)
            )
        })?;
    Ok(actual == suffix)
}

fn damaged_index(index_path: &Path, log_path: &Path) -> String {
    format!(
        "Cannot read background output index {}: it is damaged. The full log remains readable at {}.",
        display_path(index_path),
        display_path(log_path)
    )
}

fn create_private_file(path: &Path, label: &str) -> Result<File, String> {
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path).map_err(|error| {
        format!(
            "cannot create background {label} {}: {error}",
            display_path(path)
        )
    })
}

pub(super) const fn stream_bom(encoding: StreamEncoding) -> &'static [u8] {
    match encoding {
        StreamEncoding::Utf16Le => &[0xff, 0xfe],
        StreamEncoding::Utf16Be => &[0xfe, 0xff],
        StreamEncoding::Utf32Le => &[0xff, 0xfe, 0x00, 0x00],
        StreamEncoding::Utf32Be => &[0x00, 0x00, 0xfe, 0xff],
    }
}

pub(super) const fn line_ending(encoding: Option<StreamEncoding>) -> &'static [u8] {
    match encoding {
        None => b"\n",
        Some(StreamEncoding::Utf16Le) => &[b'\n', 0],
        Some(StreamEncoding::Utf16Be) => &[0, b'\n'],
        Some(StreamEncoding::Utf32Le) => &[b'\n', 0, 0, 0],
        Some(StreamEncoding::Utf32Be) => &[0, 0, 0, b'\n'],
    }
}

#[cfg(test)]
mod tests {
    use super::{
        INDEX_ENTRY_BYTES, INDEX_HEADER, LineIndexEntry, OutputLogReader, OutputLogWriter,
    };
    use crate::shell::normalize::{NormalizedEvent, StreamEncoding};
    use std::io::Write as _;

    #[cfg(windows)]
    fn mark_sparse(file: &std::fs::File) {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::System::IO::DeviceIoControl;

        const FSCTL_SET_SPARSE: u32 = 590_020;
        let mut returned = 0_u32;
        // SAFETY: the file handle is valid for the duration of the synchronous
        // call; this control code has no input or output buffers.
        let marked = unsafe {
            DeviceIoControl(
                file.as_raw_handle(),
                FSCTL_SET_SPARSE,
                std::ptr::null(),
                0,
                std::ptr::null_mut(),
                0,
                &mut returned,
                std::ptr::null_mut(),
            )
        };
        assert_ne!(
            marked,
            0,
            "failed to mark the query-complexity fixture sparse: {}",
            std::io::Error::last_os_error()
        );
    }

    #[cfg(not(windows))]
    fn mark_sparse(_file: &std::fs::File) {}

    #[test]
    fn writer_keeps_complete_long_lines_and_a_fixed_line_index() {
        let temp = tempfile::tempdir().unwrap();
        let mut writer = OutputLogWriter::new(temp.path()).unwrap();
        let payload = vec![b'x'; 400_000];
        writer.append(NormalizedEvent::Start(None)).unwrap();
        for chunk in payload.chunks(16 * 1024) {
            writer
                .append(NormalizedEvent::Bytes(chunk.to_vec()))
                .unwrap();
        }
        assert_eq!(
            writer
                .append(NormalizedEvent::LineEnd { terminated: true })
                .unwrap(),
            Some(1)
        );
        writer.finish().unwrap();

        let log = std::fs::read(temp.path().join("output.log")).unwrap();
        assert_eq!(&log[..payload.len()], payload.as_slice());
        assert_eq!(&log[payload.len()..], b"\n");
        let index = std::fs::read(temp.path().join("output.idx")).unwrap();
        assert_eq!(&index[..INDEX_HEADER.len()], INDEX_HEADER);
        assert_eq!(
            index.len() as u64,
            INDEX_HEADER.len() as u64 + INDEX_ENTRY_BYTES
        );
        assert_eq!(
            LineIndexEntry::decode(&index[INDEX_HEADER.len()..]),
            Some(LineIndexEntry {
                start: 0,
                content_end: payload.len() as u64,
                record_end: payload.len() as u64 + 1,
            })
        );
    }

    #[test]
    fn wide_stream_log_is_a_plain_bom_marked_text_file() {
        let temp = tempfile::tempdir().unwrap();
        let mut writer = OutputLogWriter::new(temp.path()).unwrap();
        writer
            .append(NormalizedEvent::Start(Some(StreamEncoding::Utf16Le)))
            .unwrap();
        writer
            .append(NormalizedEvent::Bytes(vec![b'a', 0]))
            .unwrap();
        writer
            .append(NormalizedEvent::LineEnd { terminated: true })
            .unwrap();
        writer.finish().unwrap();
        assert_eq!(
            std::fs::read(temp.path().join("output.log")).unwrap(),
            [0xff, 0xfe, b'a', 0, b'\n', 0]
        );
    }

    #[test]
    fn terminal_reader_recovers_the_log_tail_after_a_partial_index_write() {
        let temp = tempfile::tempdir().unwrap();
        let mut writer = OutputLogWriter::new(temp.path()).unwrap();
        writer.append(NormalizedEvent::Start(None)).unwrap();
        writer
            .append(NormalizedEvent::Bytes(b"first".to_vec()))
            .unwrap();
        writer
            .append(NormalizedEvent::LineEnd { terminated: true })
            .unwrap();
        writer.finish().unwrap();
        drop(writer);

        std::fs::OpenOptions::new()
            .append(true)
            .open(temp.path().join("output.log"))
            .unwrap()
            .write_all(b"second\nthird")
            .unwrap();
        let mut partial_entry = Vec::new();
        LineIndexEntry {
            start: 6,
            content_end: 12,
            record_end: 13,
        }
        .encode(&mut partial_entry);
        std::fs::OpenOptions::new()
            .append(true)
            .open(temp.path().join("output.idx"))
            .unwrap()
            .write_all(&partial_entry[..7])
            .unwrap();

        let live = OutputLogReader::open(temp.path(), false).unwrap();
        assert_eq!(live.total_lines(), 1);
        let mut terminal = OutputLogReader::open(temp.path(), true).unwrap();
        assert_eq!(terminal.total_lines(), 3);
        assert_eq!(
            terminal
                .read_range(1, 3)
                .unwrap()
                .into_iter()
                .map(|line| (line.seq, line.bytes))
                .collect::<Vec<_>>(),
            [
                (1, b"first".to_vec()),
                (2, b"second".to_vec()),
                (3, b"third".to_vec()),
            ]
        );
    }

    #[test]
    fn reader_bounds_response_memory_without_truncating_the_plain_log() {
        let temp = tempfile::tempdir().unwrap();
        let mut writer = OutputLogWriter::new(temp.path()).unwrap();
        let payload = vec![b'z'; 400_000];
        writer.append(NormalizedEvent::Start(None)).unwrap();
        writer
            .append(NormalizedEvent::Bytes(payload.clone()))
            .unwrap();
        writer.finish().unwrap();
        drop(writer);

        let mut reader = OutputLogReader::open(temp.path(), true).unwrap();
        let line = reader.read_range(1, 1).unwrap().remove(0);
        assert_eq!(line.total_bytes, payload.len() as u64);
        assert_eq!(line.bytes.len(), 64 * 1024);
        assert!(line.known_truncated);
        assert_eq!(
            std::fs::read(temp.path().join("output.log")).unwrap(),
            payload
        );
    }

    #[test]
    fn reader_answers_from_the_index_without_scanning_a_sparse_huge_log() {
        const SPARSE_LOG_BYTES: u64 = 64 * 1024 * 1024 * 1024;

        let temp = tempfile::tempdir().unwrap();
        let writer = OutputLogWriter::new(temp.path()).unwrap();
        drop(writer);
        let log_path = temp.path().join("output.log");
        let log = std::fs::OpenOptions::new()
            .write(true)
            .open(&log_path)
            .unwrap();
        mark_sparse(&log);
        log.set_len(SPARSE_LOG_BYTES).unwrap();
        let mut entry = Vec::new();
        LineIndexEntry {
            start: 0,
            content_end: SPARSE_LOG_BYTES,
            record_end: SPARSE_LOG_BYTES,
        }
        .encode(&mut entry);
        std::fs::OpenOptions::new()
            .append(true)
            .open(temp.path().join("output.idx"))
            .unwrap()
            .write_all(&entry)
            .unwrap();

        let mut reader = OutputLogReader::open(temp.path(), true).unwrap();
        let line = reader.read_range(1, 1).unwrap().remove(0);
        assert_eq!(line.total_bytes, SPARSE_LOG_BYTES);
        assert_eq!(line.bytes.len(), 64 * 1024);
        assert!(line.known_truncated);
    }

    #[test]
    fn reader_rejects_a_locally_valid_but_discontinuous_middle_index_entry() {
        let temp = tempfile::tempdir().unwrap();
        let mut writer = OutputLogWriter::new(temp.path()).unwrap();
        writer.append(NormalizedEvent::Start(None)).unwrap();
        for payload in [b"one".as_slice(), b"two", b"three", b"four"] {
            writer
                .append(NormalizedEvent::Bytes(payload.to_vec()))
                .unwrap();
            writer
                .append(NormalizedEvent::LineEnd { terminated: true })
                .unwrap();
        }
        writer.finish().unwrap();
        drop(writer);

        let index_path = temp.path().join("output.idx");
        let mut index = std::fs::read(&index_path).unwrap();
        let second_record_end = INDEX_HEADER.len() + INDEX_ENTRY_BYTES as usize + 16;
        index[second_record_end..second_record_end + 8].copy_from_slice(&7_u64.to_le_bytes());
        std::fs::write(index_path, index).unwrap();

        let mut reader = OutputLogReader::open(temp.path(), true).unwrap();
        let error = reader.read_range(2, 2).unwrap_err();
        assert!(error.contains("index"));
        assert!(error.contains("damaged"));
    }
}
