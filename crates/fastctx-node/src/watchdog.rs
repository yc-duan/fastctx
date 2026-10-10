//! The control loop must remain schedulable while jobs and network I/O wait.

use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, mpsc};

use fastctx_net::wire::{HEARTBEAT, SILENCE};
use fastctx_platform::awake::Sample;
use tokio::sync::Notify;

pub(crate) struct Watchdog {
    progress: Arc<Progress>,
    stop: mpsc::Sender<()>,
    thread: Option<std::thread::JoinHandle<()>>,
}

struct Progress {
    answered: AtomicU64,
    wake: Notify,
}

impl Watchdog {
    pub fn start(log: Arc<crate::log::Log>) -> io::Result<Self> {
        let initial = Sample::now()?;
        let progress = Arc::new(Progress { answered: AtomicU64::new(0), wake: Notify::new() });
        let (stop, stopped) = mpsc::channel();
        let watched = progress.clone();
        let thread = std::thread::Builder::new().name("node-watchdog".to_string()).spawn(move || {
            let mut previous = initial;
            let mut missed = 0;
            let mut answered = 0;
            watched.wake.notify_one();
            loop {
                match stopped.recv_timeout(HEARTBEAT) {
                    Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => return,
                    Err(mpsc::RecvTimeoutError::Timeout) => {},
                }
                let now = match Sample::now() {
                    Ok(now) => now,
                    Err(error) => {
                        log.line(format_args!("watchdog cannot read the running-time clock: {error}"));
                        std::process::exit(1);
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
                    log.line(format_args!("watchdog: the node control loop did not run for {missed} consecutive heartbeat observations; exiting for the service to restart it"));
                    // Runtime shutdown can wait forever for its stuck workers.
                    // Exit from this independent OS thread, without unwinding.
                    std::process::exit(1);
                }
                previous = now;
                answered = current;
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
