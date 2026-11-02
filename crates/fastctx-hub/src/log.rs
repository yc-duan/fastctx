//! The hub's diagnostics, one timestamped line each, in `hub.log` and on
//! standard error, where a service manager's journal keeps them. A line is
//! handed to a thread of its own to write, so logging never blocks the
//! thread that logs (`specs/world-engine.md`); the lines left when the hub
//! lets its log go still reach the file.

use std::io;
use std::path::Path;

use fastctx_platform::log_writer::LogWriter;

pub struct Log(LogWriter);

impl Log {
    pub fn open(path: &Path) -> io::Result<Self> {
        Ok(Self(LogWriter::open(path, true, "hub-log")?))
    }

    pub fn line(&self, text: impl std::fmt::Display) {
        self.0.line(format!("{} {text}\n", jiff::Timestamp::now()));
    }

    /// The last lines, oldest first.
    pub fn recent(&self) -> Vec<String> {
        self.0.recent()
    }

    /// Waits at most `within` for every line handed over so far to be
    /// written, as before the process ends.
    pub fn flush(&self, within: std::time::Duration) {
        self.0.flush(within);
    }
}
