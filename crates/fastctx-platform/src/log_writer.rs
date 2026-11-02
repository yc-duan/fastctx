//! A log written by a thread of its own (`specs/world-engine.md`, 线程的三种
//! 角色): whoever logs hands the line over and goes on. When the log falls
//! behind, lines are dropped and counted rather than making anyone wait,
//! and the next line written says how many.

use std::collections::VecDeque;
use std::io::{self, Write as _};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::rolling_log::RollingLog;

/// Lines waiting to be written before more are dropped.
const WAITING: usize = 16_384;
/// The last lines kept in memory, for a scene (`crate::scene`).
const RECENT: usize = 200;

enum Message {
    Line(String),
    Flush(mpsc::Sender<()>),
}

pub struct LogWriter {
    sender: SyncSender<Message>,
    recent: Arc<Mutex<VecDeque<String>>>,
    dropped: Arc<AtomicU64>,
    /// Lines handed over and not yet written.
    waiting: Arc<std::sync::atomic::AtomicUsize>,
    /// How many of `WAITING` are used, told in the machine's health.
    used: &'static crate::health::Bound,
}

impl LogWriter {
    /// A log at `path`, written by a thread named `name`; with `mirror`,
    /// each line also goes to standard error.
    pub fn open(path: &Path, mirror: bool, name: &str) -> io::Result<Self> {
        let log = RollingLog::open(path)?;
        let (sender, receiver) = mpsc::sync_channel(WAITING);
        let dropped = Arc::new(AtomicU64::new(0));
        let lost = dropped.clone();
        let waiting = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let written = waiting.clone();
        let named = name.to_string();
        std::thread::Builder::new().name(named.clone()).spawn(move || {
            crate::roles::set(crate::roles::Role::Blocking);
            let mut failing = false;
            while let Ok(message) = receiver.recv() {
                match message {
                    Message::Line(line) => {
                        written.fetch_sub(1, Ordering::Relaxed);
                        let missed = lost.swap(0, Ordering::Relaxed);
                        if missed > 0 {
                            let note = format!(
                                "{} {missed} log lines were dropped while the log fell behind\n",
                                jiff::Timestamp::now()
                            );
                            write(&log, note.as_bytes(), mirror, &mut failing, &named);
                        }
                        write(&log, line.as_bytes(), mirror, &mut failing, &named);
                    }
                    Message::Flush(done) => {
                        let _ = done.send(());
                    }
                }
            }
        })?;
        Ok(Self {
            sender,
            recent: Arc::new(Mutex::new(VecDeque::with_capacity(RECENT))),
            dropped,
            waiting,
            used: crate::health::bound("log.waiting"),
        })
    }

    /// Hands `line`, ending in a newline, over to be written.
    pub fn line(&self, line: String) {
        {
            let mut recent = self.recent.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            if recent.len() == RECENT {
                recent.pop_front();
            }
            recent.push_back(line.clone());
        }
        // Counted before it is handed over: the writer may write it and count
        // it off before this thread goes on, and counted after, the count
        // would go below nothing in between.
        let waiting = self.waiting.fetch_add(1, Ordering::Relaxed) + 1;
        match self.sender.try_send(Message::Line(line)) {
            Ok(()) => self.used.note(waiting, WAITING),
            Err(TrySendError::Disconnected(_)) => {
                self.waiting.fetch_sub(1, Ordering::Relaxed);
            }
            Err(TrySendError::Full(_)) => {
                self.waiting.fetch_sub(1, Ordering::Relaxed);
                self.dropped.fetch_add(1, Ordering::Relaxed);
                self.used.note(WAITING, WAITING);
            }
        }
    }

    /// Waits at most `within` for every line handed over so far to be
    /// written; whether they were.
    pub fn flush(&self, within: Duration) -> bool {
        let until = Instant::now() + within;
        let (done, written) = mpsc::channel();
        let mut message = Message::Flush(done);
        loop {
            match self.sender.try_send(message) {
                Ok(()) => break,
                Err(TrySendError::Disconnected(_)) => return false,
                Err(TrySendError::Full(back)) => {
                    if Instant::now() >= until {
                        return false;
                    }
                    message = back;
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
        }
        written.recv_timeout(until.saturating_duration_since(Instant::now())).is_ok()
    }

    /// The last lines handed over, oldest first.
    pub fn recent(&self) -> Vec<String> {
        self.recent.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).iter().cloned().collect()
    }
}

impl Drop for LogWriter {
    /// The lines handed over before the process lets its log go still
    /// reach the file.
    fn drop(&mut self) {
        self.flush(Duration::from_millis(500));
    }
}

fn write(log: &RollingLog, bytes: &[u8], mirror: bool, failing: &mut bool, name: &str) {
    match log.write(bytes) {
        Ok(()) => *failing = false,
        Err(error) => {
            if !*failing {
                eprintln!("{name}: writing diagnostics: {error}");
            }
            *failing = true;
        }
    }
    if mirror {
        let _ = io::stderr().write_all(bytes);
    }
}
