//! Starting work on the kind of thread it belongs on, and timing each slice
//! an asynchronous task takes of its thread (`specs/world-engine.md`,
//! 线程的三种角色).

use std::future::Future;
use std::panic::Location;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use tokio::task::JoinHandle;

use crate::readings::{self, Timing};
use crate::roles::{self, Role};

/// A task that keeps its thread this long at once is noted
/// (`specs/world-foundation.md`, 地基的数字).
pub const SLOW_POLL: Duration = Duration::from_millis(50);
/// How often a watched runtime is handed a tick to run.
const TICK: Duration = Duration::from_millis(100);
/// Ticks a runtime may leave unrun before it counts as held up.
const HELD_UP: Duration = Duration::from_secs(1);
/// Ticks waiting at most; past it the runtime is surely held up.
const QUEUED: usize = 64;

static POLLS: LazyLock<&'static Timing> = LazyLock::new(|| readings::timing("tasks.poll"));

/// Starts `future` as a task whose every slice on its thread is timed; a
/// slow slice names the task by what it said it does, or by where it was
/// started.
#[track_caller]
pub fn spawn<F>(future: F) -> JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    tokio::spawn(Timed { future: Box::pin(future), started_at: Location::caller() })
}

/// Runs `work` on the runtime's blocking pool, as doing what the current
/// task does, on a thread that may block.
pub fn spawn_blocking<F, R>(work: F) -> JoinHandle<R>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    tokio::task::spawn_blocking(crate::doing::carried(move || {
        let _role = roles::scoped(Role::Blocking);
        work()
    }))
}

struct Timed<F: Future> {
    future: Pin<Box<F>>,
    started_at: &'static Location<'static>,
}

impl<F: Future> Future for Timed<F> {
    type Output = F::Output;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<F::Output> {
        let started = Instant::now();
        let polled = self.future.as_mut().poll(cx);
        let took = started.elapsed();
        POLLS.record(took);
        if took >= SLOW_POLL {
            let task = match crate::doing::said_label() {
                Some(label) => label.to_string(),
                None => format!("a task started at {}:{}", self.started_at.file(), self.started_at.line()),
            };
            crate::conduct::slow_poll(&task, took);
        }
        polled
    }
}

/// How far a watched runtime has run (`watch`).
#[derive(Clone)]
pub struct Watched(Arc<AtomicU64>);

impl Watched {
    /// How many ticks the runtime has run since it was first watched: a
    /// count that stands still says its tasks get no thread.
    pub fn ran(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }
}

/// Keeps measuring how long the tasks of the current runtime, named
/// `runtime`, wait for a thread. A thread of its own hands the runtime a
/// timestamped tick every `TICK`, and a task records how long each waited to
/// be run: the timer's own slack is not counted, only the waiting. The same
/// thread sees a runtime fall behind while it is still held up, and writes
/// the scene down then, while the tasks it waits on still hold what they
/// hold.
pub fn watch(runtime: &'static str) -> Watched {
    let name: &'static str = Box::leak(format!("runtime.{runtime}.waited").into_boxed_str());
    let waited = readings::timing(name);
    let (ticks, mut arriving) = tokio::sync::mpsc::channel::<Instant>(QUEUED);
    let run = Arc::new(AtomicU64::new(0));
    let counted = run.clone();
    let watched = Watched(run.clone());
    tokio::spawn(async move {
        while let Some(handed) = arriving.recv().await {
            waited.record(handed.elapsed());
            counted.fetch_add(1, Ordering::Relaxed);
        }
    });
    let lag = crate::health::Lag::claim();
    let watching = std::thread::Builder::new().name(format!("watch-{runtime}")).spawn(move || {
        let mut handed: u64 = 0;
        // Since when the runtime has left ticks unrun, and whether the scene
        // of this time is written down.
        let mut behind: Option<(Instant, bool)> = None;
        loop {
            std::thread::sleep(TICK);
            match ticks.try_send(Instant::now()) {
                Ok(()) => handed += 1,
                Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => return,
                Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {}
            }
            let unrun = handed.saturating_sub(run.load(Ordering::Relaxed));
            if unrun > 1 {
                let (since, written) = behind.get_or_insert_with(|| {
                    let since = Instant::now() - TICK;
                    if let Some(lag) = lag {
                        lag.fell_behind(since);
                    }
                    (since, false)
                });
                if !*written && since.elapsed() >= HELD_UP {
                    *written = true;
                    crate::scene::capture(format!(
                        "the {runtime} runtime's tasks have waited {} ms for a thread",
                        since.elapsed().as_millis()
                    ));
                }
            } else if let Some((since, _)) = behind.take() {
                if let Some(lag) = lag {
                    lag.caught_up(since.elapsed());
                }
                if since.elapsed() >= HELD_UP {
                    crate::conduct::held_up(runtime, since.elapsed());
                }
            }
        }
    });
    if let Err(error) = watching {
        eprintln!("watching the {runtime} runtime: {error}");
    }
    watched
}
