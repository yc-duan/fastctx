//! Reusable isolated processes with FIFO admission and a deadline that also
//! includes queueing. Dropping a request cancels its current process.

mod lifecycle;
mod process;
mod protocol;

use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::{Duration, Instant};

use crate::child::Child;
use process::Process;

/// One kind of isolated worker process, a few of them at most.
pub struct Pool {
    settings: Arc<Settings>,
    available: Arc<tokio::sync::Semaphore>,
    shared: Arc<Shared>,
    initialization: Option<io::Error>,
}

pub struct Settings {
    /// Named in readings and in `doing`, as "ingest".
    pub name: &'static str,
    /// The fastctx executable.
    pub program: PathBuf,
    /// Its argument after `work`: `fastctx work <role>`.
    pub role: &'static str,
    /// Processes alive at once, at most. Must be nonzero.
    pub processes: usize,
    /// Bytes each may allocate beyond what it holds as it starts.
    pub memory: u64,
    /// Requests one process answers before replacement; zero also means one.
    pub recycled_after: u32,
    /// How long an idle process is kept.
    pub idle: Duration,
}

#[derive(Debug)]
pub enum Failure {
    /// No answer within the time given: the process was killed.
    Timeout,
    /// The process ended before answering: how, and its last 4 KiB of stderr.
    Died { status: String, said: String },
    /// The worker answered that it would not do it, or a frame exceeded 64 MiB.
    Refused(String),
    /// No process could be started or spoken to.
    Unavailable(io::Error),
}

struct Shared {
    state: Mutex<State>,
    changed: Condvar,
}

#[derive(Default)]
struct State {
    closed: bool,
    idle: Vec<(Instant, Process)>,
    children: Vec<Weak<Child>>,
}

impl Pool {
    pub fn new(settings: Settings) -> Arc<Self> {
        let shared = Arc::new(Shared { state: Mutex::new(State::default()), changed: Condvar::new() });
        let available = Arc::new(tokio::sync::Semaphore::new(settings.processes));
        let reaping = shared.clone();
        let retiring = available.clone();
        let idle = settings.idle;
        // This thread needs no runtime, and sleeps until the next idle expiry
        // or a change. Its state is closed explicitly when Pool is dropped.
        let initialization = std::thread::Builder::new()
            .name(format!("{} worker idle", settings.name))
            .spawn(move || reap(&reaping, &retiring, idle))
            .err();
        Arc::new(Self { available, settings: Arc::new(settings), shared, initialization })
    }

    /// Sends one request to an idle process, starting one within `processes`,
    /// and waits for its one answer. Callers past `processes` wait their turn
    /// in arrival order, their wait counted in `within`. Cancelling this future
    /// kills the process handling it. Oversized frames are refused.
    pub async fn run(&self, request: Vec<u8>, within: Duration) -> Result<Vec<u8>, Failure> {
        let began = Instant::now();
        if request.len() > protocol::MAX {
            return Err(Failure::Refused(protocol::TOO_LARGE.to_string()));
        }
        if let Some(error) = &self.initialization {
            return Err(Failure::Unavailable(io::Error::new(error.kind(), error.to_string())));
        }
        if self.settings.processes == 0 {
            return Err(Failure::Unavailable(io::Error::new(io::ErrorKind::InvalidInput, "worker processes is zero")));
        }
        let cancellation = Arc::new(Mutex::new(Cancellation::default()));
        let _cancel = Cancel(cancellation.clone());
        crate::doing::outside(format!("the {} worker", self.settings.name), within, async {
            let permit =
                self.available.clone().acquire_owned().await.map_err(io::Error::other).map_err(Failure::Unavailable)?;
            crate::readings::timing_of("worker.wait", self.settings.name).record(began.elapsed());
            let settings = self.settings.clone();
            let shared = self.shared.clone();
            crate::task::spawn_blocking(move || {
                // Keep the admission until killed I/O and startup have ended,
                // even if the asynchronous caller has already left.
                let result = if began.elapsed() >= within {
                    Err(Failure::Timeout)
                } else {
                    let started = Instant::now();
                    let result = exchange(&settings, &shared, &cancellation, &request);
                    crate::readings::timing_of("worker.run", settings.name).record(started.elapsed());
                    result
                };
                drop(permit);
                shared.changed.notify_one();
                result
            })
            .await
            .map_err(io::Error::other)
            .map_err(Failure::Unavailable)?
        })
        .await
        .map_err(|_| Failure::Timeout)?
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        let children: Vec<_> = {
            let mut state = self.shared.state.lock().expect("never poisoned");
            state.closed = true;
            state.children.iter().filter_map(Weak::upgrade).collect()
        };
        self.shared.changed.notify_one();
        for child in children {
            let _ = child.kill();
        }
    }
}

#[derive(Default)]
struct Cancellation {
    cancelled: bool,
    child: Option<Arc<Child>>,
}

struct Cancel(Arc<Mutex<Cancellation>>);

impl Drop for Cancel {
    fn drop(&mut self) {
        let child = {
            let mut cancellation = self.0.lock().expect("never poisoned");
            cancellation.cancelled = true;
            cancellation.child.take()
        };
        if let Some(child) = child {
            let _ = child.kill();
        }
    }
}

fn exchange(
    settings: &Settings,
    shared: &Shared,
    cancellation: &Mutex<Cancellation>,
    request: &[u8],
) -> Result<Vec<u8>, Failure> {
    if cancellation.lock().expect("never poisoned").cancelled {
        return Err(Failure::Timeout);
    }
    let cached = {
        let mut state = shared.state.lock().expect("never poisoned");
        if state.closed {
            return Err(Failure::Unavailable(io::Error::other("the worker pool is closed")));
        }
        state.idle.pop()
    };
    let mut process = match cached {
        Some((since, process)) if since.elapsed() < settings.idle && process.alive() => {
            attach(shared, cancellation, &process.child).map_err(Failure::Unavailable)?;
            process
        }
        cached => {
            drop(cached);
            Process::start(settings, |child| attach(shared, cancellation, child)).map_err(Failure::Unavailable)?
        }
    };
    let result = process.exchange(request);
    let reusable = {
        let mut cancellation = cancellation.lock().expect("never poisoned");
        cancellation.child = None;
        !cancellation.cancelled && result.is_ok() && process.answered < settings.recycled_after
    };
    if reusable && !settings.idle.is_zero() && process.alive() {
        let mut state = shared.state.lock().expect("never poisoned");
        if !state.closed {
            state.idle.push((Instant::now(), process));
            shared.changed.notify_one();
        }
    }
    result
}

fn attach(shared: &Shared, cancellation: &Mutex<Cancellation>, child: &Arc<Child>) -> io::Result<()> {
    {
        let mut state = shared.state.lock().expect("never poisoned");
        if state.closed {
            return Err(io::Error::other("the worker pool is closed"));
        }
        state.children.retain(|child| child.strong_count() > 0);
        if !state.children.iter().any(|known| known.as_ptr() == Arc::as_ptr(child)) {
            state.children.push(Arc::downgrade(child));
        }
    }
    let mut cancellation = cancellation.lock().expect("never poisoned");
    if cancellation.cancelled {
        return Err(io::Error::other("the worker request was cancelled"));
    }
    cancellation.child = Some(child.clone());
    Ok(())
}

fn reap(shared: &Shared, available: &Arc<tokio::sync::Semaphore>, idle: Duration) {
    let _role = crate::roles::scoped(crate::roles::Role::Blocking);
    loop {
        let mut state = shared.state.lock().expect("never poisoned");
        if state.closed {
            let processes = std::mem::take(&mut state.idle);
            drop(state);
            drop(processes);
            return;
        }
        let expired = state.idle.iter().position(|(since, process)| since.elapsed() >= idle || !process.alive());
        if let Some(index) = expired {
            // Retirement still occupies one process slot until wait confirms
            // exit. Never hold the state lock across kill or wait.
            if let Ok(permit) = available.clone().try_acquire_owned() {
                let process = state.idle.swap_remove(index);
                drop(state);
                drop(process);
                drop(permit);
                continue;
            }
        }
        let next = state.idle.iter().filter_map(|(since, _)| idle.checked_sub(since.elapsed())).min();
        match next {
            Some(next) => drop(shared.changed.wait_timeout(state, next).expect("never poisoned")),
            None => drop(shared.changed.wait(state).expect("never poisoned")),
        }
    }
}

/// A role exported by a crate and included in the executable's role tables.
/// Its entry point calls `serve`, keeping stdout exclusively for frames.
pub struct Role {
    pub name: &'static str,
    pub serve: fn(u64) -> io::Result<()>,
}

/// Runs a role from explicitly linked tables, without a runtime or global
/// registration. Each owning crate may export its own static slice of roles.
pub fn dispatch(name: &str, memory: u64, tables: &[&[Role]]) -> io::Result<()> {
    let mut found = tables.iter().flat_map(|table| table.iter()).filter(|role| role.name == name);
    let role = found.next().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "unknown worker role"))?;
    if found.next().is_some() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "duplicate worker role"));
    }
    (role.serve)(memory)
}

/// Applies the memory limit, then answers framed standard input requests one
/// at a time until it closes. Call before starting threads or a runtime. Each
/// reply consists of an outcome frame (`[0]` or `[1]`), then its payload frame.
pub fn serve(memory: u64, mut answer: impl FnMut(&[u8]) -> Result<Vec<u8>, String>) -> io::Result<()> {
    lifecycle::prepare()?;
    let _limit = crate::memory::limit(memory)?;
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    loop {
        match protocol::read(&mut input)? {
            protocol::Frame::End => return Ok(()),
            protocol::Frame::Bytes(bytes) => protocol::reply(&mut output, answer(&bytes))?,
            protocol::Frame::Oversized => {
                // The peer did not respect the frame budget. Reply without
                // allocating or reading an unbounded body, then end this pipe.
                protocol::reply(&mut output, Err(protocol::TOO_LARGE.to_string()))?;
                return Ok(());
            }
        }
    }
}
