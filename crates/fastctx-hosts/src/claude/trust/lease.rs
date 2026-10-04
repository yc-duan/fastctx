use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use filetime::FileTime;

// Claude 2.1.287–2.1.289 use proper-lockfile's 10-second stale threshold
// and renew at half that interval. These are interoperability constants.
const STALE_MS: i128 = 10_000;
const RENEW: Duration = Duration::from_millis((STALE_MS / 2) as u64);

struct State {
    mtime: FileTime,
    seconds: bool,
    failure: Option<io::Error>,
}

pub(super) struct Lease {
    path: PathBuf,
    state: Arc<Mutex<State>>,
    stop: mpsc::Sender<()>,
    worker: Option<JoinHandle<()>>,
}

impl Lease {
    pub(super) fn acquire(file: &Path) -> io::Result<Self> {
        let mut name = file.as_os_str().to_owned();
        name.push(".lock");
        let path = PathBuf::from(name);
        acquire(&path, true)?;
        let initial = (|| {
            // The +5ms probe is also used by Claude to distinguish filesystems
            // with whole-second mtimes from ones that retain milliseconds.
            set_mtime(&path, ((now_ms()? + 999) / 1000) * 1000 + 5)?;
            let mtime = mtime(&path)?;
            Ok::<_, io::Error>(State { seconds: millis(mtime) % 1000 == 0, mtime, failure: None })
        })();
        let state = match initial {
            Ok(state) => Arc::new(Mutex::new(state)),
            Err(error) => {
                let _ = fs::remove_dir(&path);
                return Err(error);
            }
        };
        let (stop, receiver) = mpsc::channel();
        let shared = Arc::clone(&state);
        let renewed = path.clone();
        let worker = thread::Builder::new().name("claude-state-lock".into()).spawn(move || {
            while let Err(mpsc::RecvTimeoutError::Timeout) = receiver.recv_timeout(RENEW) {
                let Ok(mut state) = shared.lock() else { return };
                let result = (|| {
                    check(&renewed, &state)?;
                    let now = now_ms()?;
                    let stamp = if state.seconds { ((now + 999) / 1000) * 1000 } else { now };
                    set_mtime(&renewed, stamp)?;
                    state.mtime = mtime(&renewed)?;
                    Ok(())
                })();
                if let Err(error) = result {
                    state.failure = Some(error);
                    return;
                }
            }
        });
        match worker {
            Ok(worker) => Ok(Self { path, state, stop, worker: Some(worker) }),
            Err(error) => {
                let _ = fs::remove_dir(path);
                Err(error)
            }
        }
    }

    pub(super) fn check(&self) -> io::Result<()> {
        let state = self.state.lock().map_err(|_| io::Error::other("Claude state lock thread panicked"))?;
        check(&self.path, &state)
    }

    pub(super) fn release(mut self) -> io::Result<()> {
        self.finish()
    }

    fn finish(&mut self) -> io::Result<()> {
        let Some(worker) = self.worker.take() else { return Ok(()) };
        let _ = self.stop.send(());
        worker.join().map_err(|_| io::Error::other("Claude state lock thread panicked"))?;
        self.check()?;
        fs::remove_dir(&self.path)
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        let _ = self.finish();
    }
}

fn acquire(path: &Path, allow_stale: bool) -> io::Result<()> {
    match fs::create_dir(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists && allow_stale => match mtime(path) {
            Ok(stamp) if millis(stamp) < now_ms()? - STALE_MS => {
                match fs::remove_dir(path) {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error),
                }
                acquire(path, false)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => acquire(path, false),
            Err(error) => Err(error),
            Ok(_) => Err(locked()),
        },
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Err(locked()),
        Err(error) => Err(error),
    }
}

fn locked() -> io::Error {
    io::Error::new(io::ErrorKind::WouldBlock, "Claude global state lock is held")
}

fn check(path: &Path, state: &State) -> io::Result<()> {
    if let Some(error) = &state.failure {
        return Err(io::Error::new(error.kind(), format!("Claude state lock renewal failed: {error}")));
    }
    if millis(mtime(path)?) != millis(state.mtime) || millis(state.mtime) < now_ms()? - STALE_MS {
        return Err(io::Error::other("Claude global state lock was lost or expired"));
    }
    Ok(())
}

fn mtime(path: &Path) -> io::Result<FileTime> {
    Ok(FileTime::from_last_modification_time(&fs::metadata(path)?))
}

fn millis(stamp: FileTime) -> i128 {
    i128::from(stamp.unix_seconds()) * 1000 + i128::from(stamp.nanoseconds() / 1_000_000)
}

fn now_ms() -> io::Result<i128> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH).map_err(io::Error::other)?.as_millis() as i128)
}

fn set_mtime(path: &Path, stamp: i128) -> io::Result<()> {
    filetime::set_file_mtime(path, FileTime::from_unix_time((stamp / 1000) as i64, (stamp % 1000) as u32 * 1_000_000))
}
