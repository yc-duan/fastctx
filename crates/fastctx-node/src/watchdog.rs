//! The control loop must remain schedulable while jobs and network I/O wait,
//! and so must the runtime's worker threads that carry every other task.

use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, mpsc};

use std::time::Duration;

use fastctx_net::wire::{HEARTBEAT, SILENCE};
use fastctx_platform::awake::Sample;
use tokio::sync::Notify;

/// How often a task on the worker threads says it ran.
const BEAT: Duration = Duration::from_secs(2);

/// How long the worker threads may run nothing at all before the node ends.
/// Far longer than any wait a task should block a worker for; the node of
/// 2026-10-17 had every worker blocked for twenty-five minutes while its
/// control loop answered on the main thread.
const WORKERS_SILENT: Duration = Duration::from_secs(90);

pub(crate) struct Watchdog {
    progress: Arc<Progress>,
    stop: mpsc::Sender<()>,
    thread: Option<std::thread::JoinHandle<()>>,
}

struct Progress {
    answered: AtomicU64,
    wake: Notify,
    /// Beats of a task on the worker threads (`BEAT`).
    workers: AtomicU64,
}

impl Watchdog {
    pub fn start() -> io::Result<Self> {
        let initial = Sample::now()?;
        let progress =
            Arc::new(Progress { answered: AtomicU64::new(0), wake: Notify::new(), workers: AtomicU64::new(0) });
        // A task the worker threads run like any other: when it stops
        // beating, nothing scheduled there runs, whatever the control loop
        // on the main thread still answers.
        let beating = progress.clone();
        tokio::spawn(async move {
            let mut every = tokio::time::interval(BEAT);
            every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                every.tick().await;
                beating.workers.fetch_add(1, Ordering::Release);
            }
        });
        let (stop, stopped) = mpsc::channel();
        let watched = progress.clone();
        let thread = std::thread::Builder::new().name("node-watchdog".to_string()).spawn(move || {
            let mut previous = initial;
            let mut missed = 0;
            let mut answered = 0;
            let mut beats = 0;
            let mut silent = 0;
            watched.wake.notify_one();
            loop {
                match stopped.recv_timeout(HEARTBEAT) {
                    Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => return,
                    Err(mpsc::RecvTimeoutError::Timeout) => {},
                }
                let now = match Sample::now() {
                    Ok(now) => now,
                    Err(error) => {
                        crate::ending::with_reason(1, format!("watchdog cannot read the running-time clock: {error}"));
                    }
                };
                // 2026-10-03: this detects a failed responsiveness contract,
                // not a proof of deadlock. The existing wire contract gives
                // H = 10 s and S = 3H = 30 s. The accept loop does no jobs or
                // remote I/O: it only dispatches clients and answers this
                // challenge. Three consecutive observations without a single
                // dispatch or acknowledgement establish no control progress
                // across the interval that also makes a node offline.
                //
                // Work duration, hub reachability and CPU use are not evidence
                // of a stall. No job has a watchdog deadline. Sleep is excluded
                // by the OS clocks. If this observer itself missed a complete
                // opportunity (elapsed >= 2H), discard the observation window
                // and issue a fresh challenge. A long process stop therefore
                // cannot spend its whole budget while nobody ran. This is a
                // responsiveness policy, not a real-time scheduler guarantee:
                // selective starvation or repeated shorter process stops can
                // still prevent the loop running throughout three observations.
                let interrupted = now.suspended_since(&previous)
                    || now.awake.saturating_sub(previous.awake) >= HEARTBEAT * 2;
                let current = watched.answered.load(Ordering::Acquire);
                if interrupted || current != answered {
                    missed = 0;
                } else {
                    missed += 1;
                }
                if HEARTBEAT * missed >= SILENCE {
                    crate::ending::with_reason(1, format!("watchdog: the node control loop did not run for {missed} consecutive heartbeat observations; exiting for the service to restart it"));
                }
                // The worker threads, by the same observations: every one of
                // them blocked in a synchronous wait leaves the tasks that
                // answer calls, talk to the hub and watch the node unrun.
                let beat = watched.workers.load(Ordering::Acquire);
                if interrupted || beat != beats {
                    silent = 0;
                } else {
                    silent += 1;
                }
                if HEARTBEAT * silent >= WORKERS_SILENT {
                    crate::ending::with_reason(1, format!("watchdog: no task ran on the node's worker threads for {silent} consecutive heartbeat observations; exiting for the service to restart it"));
                }
                previous = now;
                answered = current;
                beats = beat;
                watched.wake.notify_one();
            }
        })?;
        Ok(Self { progress, stop, thread: Some(thread) })
    }

    /// Called by the actual accept loop, never by a separate healthy task.
    pub async fn requested(&self) {
        self.progress.wake.notified().await;
        self.progress();
    }

    /// Dispatching a real client is also proof that this loop ran.
    pub fn progress(&self) {
        self.progress.answered.fetch_add(1, Ordering::Release);
    }
}

impl Drop for Watchdog {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(thread) = self.thread.take() {
            thread.join().expect("the watchdog thread panicked");
        }
    }
}
