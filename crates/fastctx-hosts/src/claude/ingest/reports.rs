//! A report's parts are consecutive bytes of one JSON value, not JSONL rows.
//! This reader never acknowledges or removes a report: that follows a committed
//! consumer receipt, after all pending dependencies and events are durable.
use fastctx_platform::file_read;
use fastctx_types::{
    file_identity::{FileChangeStamp, FileIdentity},
    sync::cursor::{ByteCount, Count},
};
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::{self, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "layout", rename_all = "snake_case", deny_unknown_fields)]
pub enum Layout {
    Inline,
    Parts { count: Count },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Position {
    pub part: Count,
    pub offset: ByteCount,
    pub total: ByteCount,
    pub current: Option<ReportStamp>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReportStamp {
    pub identity: FileIdentity,
    pub change: FileChangeStamp,
}

/// Zero bytes with complete=false advances across one part boundary. The caller
/// may commit that progress before reading again, even for empty parts.
pub struct ReportWindow {
    pub bytes: usize,
    pub complete: bool,
}
pub struct ReportReader {
    path: PathBuf,
    layout: Layout,
    stamp: ReportStamp,
    position: Position,
    current: Option<(File, ReportStamp)>,
}
impl ReportReader {
    /// `path` comes from an already claimed plugin instance. `layout` is decoded
    /// from that report's bounded outer header, never guessed from a directory.
    pub fn open(path: &Path, layout: Layout, expected: Option<&ReportStamp>, position: Position) -> io::Result<Self> {
        if !path.with_extension("done").try_exists()? {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        let file = file_read::open(path)?;
        let snapshot = file_read::snapshot(&file, &file.metadata()?)?;
        let stamp = ReportStamp { identity: snapshot.identity(), change: snapshot.change_stamp()? };
        if expected.is_some_and(|old| old != &stamp) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "report changed before resumption"));
        }
        let parts = match layout {
            Layout::Inline => 1,
            Layout::Parts { count } => count.0,
        };
        if position.part.0 > parts || position.part.0 == parts && position.offset.0 != 0 {
            return Err(io::Error::other("report position is outside its parts"));
        }
        Ok(Self { path: path.to_owned(), layout, stamp, position, current: None })
    }
    pub fn stamp(&self) -> &ReportStamp {
        &self.stamp
    }
    pub fn position(&self) -> &Position {
        &self.position
    }
    fn parts(&self) -> u64 {
        match self.layout {
            Layout::Inline => 1,
            Layout::Parts { count } => count.0,
        }
    }
    fn part_path(&self) -> PathBuf {
        match self.layout {
            Layout::Inline => self.path.clone(),
            Layout::Parts { .. } => self.path.with_extension("parts").join(self.position.part.0.to_string()),
        }
    }
    pub fn read_window(&mut self, buffer: &mut [u8]) -> io::Result<ReportWindow> {
        if buffer.is_empty() || buffer.len() > super::lexical::WINDOW_BYTES {
            return Err(io::Error::other("report read requires one bounded window"));
        }
        if self.position.part.0 == self.parts() {
            self.verify_manifest()?;
            return Ok(ReportWindow { bytes: 0, complete: true });
        }
        if self.current.is_none() {
            let mut file = file_read::open(&self.part_path())?;
            let snapshot = file_read::snapshot(&file, &file.metadata()?)?;
            if self.position.offset.0 > snapshot.size {
                return Err(io::Error::new(io::ErrorKind::Interrupted, "report part was truncated"));
            }
            let part_stamp = ReportStamp { identity: snapshot.identity(), change: snapshot.change_stamp()? };
            if self.position.current.as_ref().is_some_and(|old| old != &part_stamp)
                || self.position.offset.0 > 0 && self.position.current.is_none()
            {
                return Err(io::Error::new(io::ErrorKind::Interrupted, "report part changed before resumption"));
            }
            file.seek(SeekFrom::Start(self.position.offset.0))?;
            self.position.current = Some(part_stamp.clone());
            self.current = Some((file, part_stamp));
        }
        let part_path = self.part_path();
        let (file, before) = self.current.as_mut().ok_or_else(|| io::Error::other("report part failed to open"))?;
        let count = file.read(buffer)?;
        let snapshot = file_read::snapshot(file, &file.metadata()?)?;
        if snapshot.identity() != before.identity || snapshot.change_stamp()? != before.change {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "report part changed while reading"));
        }
        if count > 0 {
            self.position.offset.0 = self
                .position
                .offset
                .0
                .checked_add(count as u64)
                .ok_or_else(|| io::Error::other("report offset overflow"))?;
            self.position.total.0 = self
                .position
                .total
                .0
                .checked_add(count as u64)
                .ok_or_else(|| io::Error::other("report length overflow"))?;
            return Ok(ReportWindow { bytes: count, complete: false });
        }
        let path_file = file_read::open(&part_path)?;
        let path_stamp = file_read::snapshot(&path_file, &path_file.metadata()?)?;
        if path_stamp.identity() != before.identity || path_stamp.change_stamp()? != before.change {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "report part path was replaced"));
        }
        self.current = None;
        self.position.current = None;
        self.position.part.0 += 1;
        self.position.offset = ByteCount(0);
        Ok(ReportWindow { bytes: 0, complete: false })
    }
    fn verify_manifest(&self) -> io::Result<()> {
        let file = file_read::open(&self.path)?;
        let now = file_read::snapshot(&file, &file.metadata()?)?;
        if now.identity() != self.stamp.identity
            || now.change_stamp()? != self.stamp.change
            || !self.path.with_extension("done").try_exists()?
        {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "report manifest or completion marker changed"));
        }
        Ok(())
    }
}
/// A dependency is a single report filename, not a path supplied by a host.
pub fn dependency_name(name: &str) -> io::Result<&str> {
    if name.is_empty()
        || matches!(name, "." | "..")
        || name.contains(['/', '\\', ':', '\0'])
        || !name.ends_with(".json")
    {
        return Err(io::Error::other("invalid report dependency name"));
    }
    Ok(name)
}
