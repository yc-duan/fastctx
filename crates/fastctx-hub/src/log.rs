//! The hub's diagnostics, one timestamped line each, in `hub.log` and on
//! standard error, where a service manager's journal keeps them.

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
        let _ = io::stderr().write_all(line.as_bytes());
    }
}
