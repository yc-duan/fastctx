//! The hub's diagnostics, one timestamped line each, in `hub.log` and on
//! standard error, where a service manager keeps them — unless standard error
//! is systemd's journal, which forwards to syslog: there `hub.log` alone keeps
//! them, not three times over (Hangzhou's disk filled so). A line is
//! handed to a thread of its own to write, so logging never blocks the
//! thread that logs (`specs/world-engine.md`); the lines left when the hub
//! lets its log go still reach the file.

use std::io;
use std::path::Path;

use fastctx_platform::log_writer::LogWriter;

pub struct Log(LogWriter);

impl Log {
    pub fn open(path: &Path) -> io::Result<Self> {
        Ok(Self(LogWriter::open(path, !journal(), "hub-log")?))
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

/// Whether standard error is the journal of the systemd unit running the hub:
/// `JOURNAL_STREAM` names that stream's device and inode (systemd.exec(5)), and
/// a child of a process run so inherits the variable with another stream.
fn journal() -> bool {
    #[cfg(target_os = "linux")]
    {
        use std::os::fd::AsFd;
        use std::os::unix::fs::MetadataExt;
        let Some(named) = std::env::var_os("JOURNAL_STREAM") else { return false };
        let Some((device, inode)) = named.to_str().and_then(|named| named.split_once(':')) else { return false };
        let (Ok(device), Ok(inode)) = (device.parse::<u64>(), inode.parse::<u64>()) else { return false };
        let Ok(stream) = std::io::stderr().as_fd().try_clone_to_owned() else { return false };
        std::fs::File::from(stream).metadata().is_ok_and(|stream| stream.dev() == device && stream.ino() == inode)
    }
    #[cfg(not(target_os = "linux"))]
    false
}
