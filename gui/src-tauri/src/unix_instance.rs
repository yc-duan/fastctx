//! One Fastctx window for this user on macOS and Linux, as the named pipe
//! keeps one on Windows (`desktop_presence::instance`). The process that
//! locks `instance.lock` in the application's data folder listens on the
//! socket beside it; a later one, such as the app a repair reopens while
//! this one still shows the repair, or the program started directly, asks
//! it to bring its window forward and exits. A launch through the Dock or
//! Finder reaches the running app on its own.

use std::io::{BufRead as _, BufReader, Write as _};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use tauri::Manager as _;

const OPEN: &str = "open";
const SHOWN: &str = "shown";
/// How long a later process tries to reach the first, which may still be
/// binding its socket, and waits for its answer.
const WAIT: Duration = Duration::from_secs(5);

pub(crate) struct Instance {
    /// Held for the life of the process; the system lets go of it however
    /// the process ends.
    lock: std::fs::File,
    listener: UnixListener,
}

/// Set once this process is ending to start its successor, as after an
/// update: a later process asking is told so, and waits for the lock.
static LEAVING: AtomicBool = AtomicBool::new(false);

/// `Some` when this process is the first and shows the window, `None` when
/// the first one was asked to show its own and this one should exit.
pub(crate) fn enter(identifier: &str) -> Result<Option<Instance>, String> {
    let folder = dirs::data_dir().ok_or("this user has no data folder")?.join(identifier);
    std::fs::create_dir_all(&folder).map_err(|e| format!("creating {}: {e}", folder.display()))?;
    let name = if cfg!(debug_assertions) { "instance-development" } else { "instance" };
    let socket = folder.join(format!("{name}.sock"));
    let path = folder.join(format!("{name}.lock"));
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(|e| format!("opening {}: {e}", path.display()))?;
    let started = Instant::now();
    loop {
        match lock.try_lock() {
            Ok(()) => return listen(lock, &socket).map(Some),
            Err(std::fs::TryLockError::WouldBlock) => {}
            Err(std::fs::TryLockError::Error(error)) => return Err(format!("locking {}: {error}", path.display())),
        }
        match ask(&socket) {
            Ok(true) => return Ok(None),
            // Not listening yet, or ending to start this one: the lock is
            // tried again.
            Ok(false) | Err(_) if started.elapsed() < WAIT => std::thread::sleep(Duration::from_millis(100)),
            Ok(false) => return Err("the Fastctx already running is closing".into()),
            Err(error) => return Err(error),
        }
    }
}

fn listen(lock: std::fs::File, socket: &Path) -> Result<Instance, String> {
    // A socket left by a process that ended is in the way.
    match std::fs::remove_file(socket) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("removing {}: {error}", socket.display())),
    }
    let listener = UnixListener::bind(socket).map_err(|e| format!("listening at {}: {e}", socket.display()))?;
    Ok(Instance { lock, listener })
}

/// Whether the process already running showed its window when asked.
fn ask(socket: &Path) -> Result<bool, String> {
    let stream = UnixStream::connect(socket)
        .map_err(|e| format!("reaching the Fastctx already running at {}: {e}", socket.display()))?;
    let answer = (|| {
        stream.set_read_timeout(Some(WAIT))?;
        stream.set_write_timeout(Some(WAIT))?;
        writeln!(&stream, "{OPEN}")?;
        let mut answer = String::new();
        BufReader::new(&stream).read_line(&mut answer)?;
        Ok::<_, std::io::Error>(answer)
    })()
    .map_err(|e| format!("asking the Fastctx already running to show its window: {e}"))?;
    Ok(answer.trim() == SHOWN)
}

/// This process is ending to start another in its place.
pub(crate) fn leaving() {
    LEAVING.store(true, Ordering::SeqCst);
}

impl Instance {
    /// Answers later processes for as long as this one runs.
    pub(crate) fn serve(self, app: tauri::AppHandle) {
        let serving = app.clone();
        let served = std::thread::Builder::new().name("fastctx-instance".into()).spawn(move || {
            let app = serving;
            let _lock = self.lock;
            for stream in self.listener.incoming() {
                let Ok(stream) = stream else { continue };
                let _ = stream.set_read_timeout(Some(WAIT));
                let _ = stream.set_write_timeout(Some(WAIT));
                let mut line = String::new();
                if BufReader::new(&stream).read_line(&mut line).is_err() || line.trim() != OPEN {
                    continue;
                }
                let handle = app.clone();
                let answer = if LEAVING.load(Ordering::SeqCst) {
                    "leaving"
                } else {
                    match app.run_on_main_thread(move || reveal(&handle)) {
                        Ok(()) => SHOWN,
                        Err(_) => "leaving",
                    }
                };
                let _ = writeln!(&stream, "{answer}");
            }
        });
        if let Err(error) = served {
            crate::startup::warning(&app, "single_instance", error.to_string());
        }
    }
}

fn reveal(app: &tauri::AppHandle) {
    let Some(window) = app.get_webview_window("main") else { return };
    for (step, done) in [
        ("unminimize_window", window.unminimize()),
        ("show_window", window.show()),
        ("focus_window", window.set_focus()),
    ] {
        if let Err(error) = done {
            crate::startup::warning(app, step, error.to_string());
        }
    }
}

pub(crate) fn plugin(
    instance: Result<Instance, String>,
) -> tauri::plugin::TauriPlugin<tauri::Wry> {
    tauri::plugin::Builder::new("single-instance")
        .setup(move |app, _| {
            match instance {
                Ok(instance) => instance.serve(app.clone()),
                // Opened anyway: a window that may be one of two beats none.
                Err(error) => crate::startup::warning(app, "single_instance", error),
            }
            Ok(())
        })
        .build()
}
