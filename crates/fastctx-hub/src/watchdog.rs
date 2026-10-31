//! Independent observation of both runtimes and the lock registry. A
//! blocked business runtime cannot hide behind a healthy transport loop.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Weak, mpsc};
use std::time::{Duration, Instant};

use fastctx_platform::doing;

#[derive(Default)]
pub(crate) struct Health {
    pub network_delay: crate::measure::Histogram,
    pub work_delay: crate::measure::Histogram,
    network: AtomicU64,
    work: AtomicU64,
    pub stalls: AtomicU64,
}

pub(crate) struct Watchdog {
    stop: mpsc::Sender<()>,
    thread: Option<std::thread::JoinHandle<()>>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl Watchdog {
    pub(crate) fn start(hub: &Arc<crate::Hub>) -> std::io::Result<Self> {
        let log = hub.log.clone();
        doing::set_reporter(move |line| log.line(line));
        let (stop, stopped) = mpsc::channel();
        let network = hub.network.spawn(beat(Arc::downgrade(hub), true));
        let work = tokio::spawn(beat(Arc::downgrade(hub), false));
        let weak = Arc::downgrade(hub);
        let thread = std::thread::Builder::new().name("hub-watchdog".into()).spawn(move || {
            let mut previous = fastctx_platform::awake::Sample::now().ok();
            let mut before = [0, 0];
            let mut silent = [0_u64, 0];
            let mut circle: Option<(Vec<String>, Instant)> = None;
            let mut told = std::collections::HashMap::new();
            while matches!(stopped.recv_timeout(Duration::from_secs(10)), Err(mpsc::RecvTimeoutError::Timeout)) {
                let Some(hub) = weak.upgrade() else { break };
                let now_sample = fastctx_platform::awake::Sample::now().ok();
                let suspended =
                    previous.as_ref().zip(now_sample.as_ref()).is_some_and(|(before, now)| now.suspended_since(before));
                previous = now_sample;
                let current = [hub.watch.network.load(Ordering::Acquire), hub.watch.work.load(Ordering::Acquire)];
                for i in 0..2 {
                    silent[i] = if !suspended && current[i] == before[i] { silent[i] + 10 } else { 0 };
                    if silent[i] >= if i == 0 { 30 } else { 90 } {
                        fatal(
                            &hub,
                            &format!(
                                "watchdog: {} runtime made no progress for {} seconds",
                                if i == 0 { "network" } else { "work" },
                                silent[i]
                            ),
                        );
                    }
                }
                before = current;
                let snapshot = doing::snapshot(Duration::from_secs(30), Duration::from_secs(30));
                let now = Instant::now();
                told.retain(|key, _| snapshot.stuck.iter().any(|item| &item.key == key));
                for item in &snapshot.stuck {
                    if told
                        .get(&item.key)
                        .is_none_or(|at: &Instant| now.duration_since(*at) >= Duration::from_secs(600))
                    {
                        told.insert(item.key.clone(), now);
                        hub.log.line(format_args!(
                            "stuck for {} s: {} {}",
                            item.since.as_secs(),
                            item.doing,
                            item.what
                        ));
                        leave_scene(&hub, "stuck work");
                    }
                }
                if let Some(cycle) = snapshot.cycles.first() {
                    let mut links = cycle.links.clone();
                    links.sort();
                    match &circle {
                        Some((known, since)) if *known == links && since.elapsed() >= Duration::from_secs(60) => {
                            fatal(&hub, &format!("deadlock: {}", links.join("; ")))
                        }
                        Some((known, _)) if *known == links => {}
                        _ => {
                            hub.log.line(format_args!("tasks wait in a circle: {}", links.join("; ")));
                            circle = Some((links, now));
                        }
                    }
                } else {
                    circle = None;
                }
            }
        })?;
        Ok(Self { stop, thread: Some(thread), tasks: vec![network, work] })
    }
}

async fn beat(hub: Weak<crate::Hub>, network: bool) {
    let spacing = Duration::from_millis(50);
    loop {
        let due = tokio::time::Instant::now() + spacing;
        tokio::time::sleep_until(due).await;
        let Some(hub) = hub.upgrade() else { return };
        let delay = due.elapsed();
        let (count, histogram) = if network {
            (&hub.watch.network, &hub.watch.network_delay)
        } else {
            (&hub.watch.work, &hub.watch.work_delay)
        };
        count.fetch_add(1, Ordering::Release);
        histogram.record(delay);
        if delay > Duration::from_secs(1) {
            hub.watch.stalls.fetch_add(1, Ordering::Relaxed);
            let weak = Arc::downgrade(&hub);
            tokio::task::spawn_blocking(move || {
                if let Some(hub) = weak.upgrade() {
                    leave_scene(&hub, "runtime delayed");
                }
            });
        }
    }
}

fn leave_scene(hub: &crate::Hub, why: &str) {
    let snapshot = doing::snapshot(Duration::from_secs(30), Duration::from_secs(30));
    let path = hub.diagnostic_dir.join(format!("stuck-{}.json", hub.watch.stalls.load(Ordering::Relaxed) % 4));
    let text = serde_json::json!({
        "at": jiff::Timestamp::now(), "reason": why,
        "network_delay": hub.watch.network_delay.snapshot(), "work_delay": hub.watch.work_delay.snapshot(),
        "stuck": snapshot.stuck.iter().map(|item| format!("{}: {} ({} s)", item.doing, item.what, item.since.as_secs())).collect::<Vec<_>>(),
        "cycles": snapshot.cycles.iter().map(|cycle| &cycle.links).collect::<Vec<_>>(),
    });
    if let Ok(bytes) = serde_json::to_vec_pretty(&text) {
        let _ = fastctx_platform::publish(&path, &bytes, fastctx_platform::Permissions::Private);
    }
}

fn fatal(hub: &crate::Hub, reason: &str) -> ! {
    leave_scene(hub, reason);
    hub.log.line(reason);
    eprintln!("fastctx hub: {reason}; exiting for its service to restart it");
    std::process::exit(1)
}

impl Drop for Watchdog {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        for task in &self.tasks {
            task.abort();
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
