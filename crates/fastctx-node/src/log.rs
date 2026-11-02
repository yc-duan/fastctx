//! The node's own diagnostics, one timestamped line each, in `node.log`.
//!
//! The node usually runs with no terminal; what goes wrong in it is
//! written here, where `fastctx` commands and a person can read it. A line
//! is handed to a thread of its own to write, so logging never blocks the
//! thread that logs (`specs/world-engine.md`).

use std::io;
use std::path::Path;
use std::time::Duration;

use fastctx_platform::log_writer::LogWriter;

pub struct Log(LogWriter);

impl Log {
    pub fn open(path: &Path) -> io::Result<Self> {
        Ok(Self(LogWriter::open(path, false, "node-log")?))
    }

    pub fn line(&self, text: impl std::fmt::Display) {
        self.0.line(format!("{} {text}\n", jiff::Timestamp::now()));
    }

    /// Waits at most `within` for the lines so far to reach the file, as
    /// before the process ends.
    pub fn flush(&self, within: Duration) -> bool {
        self.0.flush(within)
    }

    /// The last lines, oldest first.
    pub fn recent(&self) -> Vec<String> {
        self.0.recent()
    }
}
