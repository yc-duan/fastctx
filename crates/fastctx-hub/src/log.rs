//! The hub's diagnostics, one timestamped line each, in `hub.log` and on
//! standard error, where a service manager's journal keeps them.

use std::io::{self, Write as _};
use std::path::Path;

pub struct Log(fastctx_platform::rolling_log::RollingLog);

impl Log {
    pub fn open(path: &Path) -> io::Result<Self> {
        Ok(Self(fastctx_platform::rolling_log::RollingLog::open(path)?))
    }

    pub fn line(&self, text: impl std::fmt::Display) {
        let line = format!("{} {text}\n", jiff::Timestamp::now());
        if let Err(error) = self.0.write(line.as_bytes()) {
            eprintln!("fastctx hub: writing diagnostics: {error}");
        }
        let _ = io::stderr().write_all(line.as_bytes());
    }
}
