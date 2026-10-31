//! Bounded diagnostics handed to a dedicated writer, never written by a
//! member's connection or a business coordinator.

use std::io::{self, Write as _};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, SyncSender};

pub struct Log {
    send: SyncSender<String>,
    dropped: Arc<AtomicU64>,
}

impl Log {
    pub fn open(path: &Path) -> io::Result<Self> {
        let file = fastctx_platform::rolling_log::RollingLog::open(path)?;
        let (send, receive) = mpsc::sync_channel::<String>(4096);
        let dropped = Arc::new(AtomicU64::new(0));
        let lost = dropped.clone();
        std::thread::Builder::new().name("hub-log".into()).spawn(move || {
            while let Ok(line) = receive.recv() {
                let count = lost.swap(0, Ordering::Relaxed);
                if count != 0 {
                    let notice = format!("{} diagnostics queue full: {count} lines omitted\n", jiff::Timestamp::now());
                    let _ = file.write(notice.as_bytes());
                    let _ = io::stderr().write_all(notice.as_bytes());
                }
                if let Err(error) = file.write(line.as_bytes()) {
                    eprintln!("fastctx hub: writing diagnostics: {error}");
                }
                let _ = io::stderr().write_all(line.as_bytes());
            }
        })?;
        Ok(Self { send, dropped })
    }

    pub fn line(&self, text: impl std::fmt::Display) {
        let line = format!("{} {text}\n", jiff::Timestamp::now());
        if self.send.try_send(line).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}
