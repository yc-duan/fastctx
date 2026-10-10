//! The node's own diagnostics, one timestamped line each, in `node.log`.
//!
//! The node usually runs with no terminal; what goes wrong in it is
//! written here, where `fastctx` commands and a person can read it.

use std::io;
use std::path::Path;

pub struct Log(fastctx_platform::rolling_log::RollingLog);

impl Log {
    pub fn open(path: &Path) -> io::Result<Self> {
        Ok(Self(fastctx_platform::rolling_log::RollingLog::open(path)?))
    }

    pub fn line(&self, text: impl std::fmt::Display) {
        let line = format!("{} {text}\n", jiff::Timestamp::now());
        if let Err(error) = self.0.write(line.as_bytes()) {
            eprintln!("fastctx node: writing diagnostics: {error}");
        }
    }
}
