//! The node's own diagnostics, one timestamped line each, in `node.log`.
//!
//! The node usually runs with no terminal; what goes wrong in it is
//! written here, where `fastctx` commands and a person can read it.

use std::fs::{File, OpenOptions};
use std::io::{self, Write as _};
use std::path::Path;
use std::sync::Mutex;

pub struct Log(Mutex<File>);

impl Log {
    pub fn open(path: &Path) -> io::Result<Self> {
        Ok(Self(Mutex::new(OpenOptions::new().create(true).append(true).open(path)?)))
    }

    pub fn line(&self, text: impl std::fmt::Display) {
        let line = format!("{} {text}\n", jiff::Timestamp::now());
        let mut file = self.0.lock().expect("never poisoned");
        // A log that cannot be written has nowhere to report that.
        let _ = file.write_all(line.as_bytes());
    }
}
