//! The hub watching itself (`specs/world-engine.md`, 中枢): how far each of
//! its runtimes ran, as the platform measures them
//! (`fastctx_platform::task::watch`), and what its tasks wait for and hold
//! (`fastctx_platform::doing`). What has gone on too long goes in its log,
//! once and then again every ten minutes while it lasts, and counts in its
//! health. A runtime that does not run at all for long, or tasks that wait on
//! each other in a circle for a minute, end the hub for its service to start
//! it again: a hub whose heartbeats are answered while its work stands still
//! is worse than one started again. It looks from a thread of its own, which
//! a stuck runtime cannot hold up.

use std::collections::HashMap;
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use fastctx_platform::doing;
use fastctx_platform::task::Watched;

/// How often the hub looks at itself.
const LOOK: Duration = Duration::from_secs(10);
/// A wait for a lock, and a lock held, longer than this is told.
const WAITS: Duration = Duration::from_secs(30);
const HOLDS: Duration = Duration::from_secs(30);
/// What is still stuck is told again after this long.
const AGAIN: Duration = Duration::from_secs(600);
/// A circle still there this long after it was first seen ends the hub.
const CIRCLE: Duration = Duration::from_secs(60);
/// How long each runtime may not run at all before it ends the hub: the
/// network's lines are given up by their machines sooner than that.
const NETWORK_STILL: Duration = Duration::from_secs(30);
const WORK_STILL: Duration = Duration::from_secs(90);

pub(crate) struct Watchdog {
    stop: mpsc::Sender<()>,
    thread: Option<std::thread::JoinHandle<()>>,
}

/// One runtime as the watchdog follows it.
struct Followed {
    name: &'static str,
    watched: Watched,
    still: Duration,
    ran: u64,
    quiet: Duration,
}

impl Watchdog {
    pub(crate) fn start(hub: &Arc<crate::Hub>, work: Watched, network: Watched) -> std::io::Result<Self> {
        let log = hub.log.clone();
        doing::set_reporter(move |line| log.line(line));
        let (stop, stopped) = mpsc::channel();
        let weak = Arc::downgrade(hub);
        let thread = std::thread::Builder::new().name("hub-watchdog".into()).spawn(move || {
            let mut runtimes = [
                Followed {
                    name: "network",
                    ran: network.ran(),
                    watched: network,
                    still: NETWORK_STILL,
                    quiet: Duration::ZERO,
                },
                Followed { name: "work", ran: work.ran(), watched: work, still: WORK_STILL, quiet: Duration::ZERO },
            ];
            let mut previous = fastctx_platform::awake::Sample::now().ok();
            let mut told: HashMap<String, Instant> = HashMap::new();
            let mut circle: Option<(Vec<String>, Instant)> = None;
            while matches!(stopped.recv_timeout(LOOK), Err(mpsc::RecvTimeoutError::Timeout)) {
                let Some(hub) = weak.upgrade() else { break };
                // A machine that slept stood still with every runtime on it.
                let now_sample = fastctx_platform::awake::Sample::now().ok();
                let suspended =
                    previous.as_ref().zip(now_sample.as_ref()).is_some_and(|(before, now)| now.suspended_since(before));
                previous = now_sample;
                for runtime in &mut runtimes {
                    let ran = runtime.watched.ran();
                    runtime.quiet =
                        if !suspended && ran == runtime.ran { runtime.quiet + LOOK } else { Duration::ZERO };
                    runtime.ran = ran;
                    if runtime.quiet >= runtime.still {
                        fatal(
                            &hub,
                            &format!("the {} runtime ran nothing for {} s", runtime.name, runtime.quiet.as_secs()),
                        );
                    }
                }
                let snapshot = doing::snapshot(WAITS, HOLDS);
                fastctx_platform::health::set_stuck(snapshot.stuck.len());
                let now = Instant::now();
                told.retain(|key, _| snapshot.stuck.iter().any(|item| &item.key == key));
                for item in &snapshot.stuck {
                    if told.get(&item.key).is_none_or(|at| now.duration_since(*at) >= AGAIN) {
                        told.insert(item.key.clone(), now);
                        hub.log.line(format_args!(
                            "stuck for {} s: {} {}",
                            item.since.as_secs(),
                            item.doing,
                            item.what
                        ));
                        fastctx_platform::scene::capture(format!(
                            "stuck for {} s: {}",
                            item.since.as_secs(),
                            item.doing
                        ));
                    }
                }
                match snapshot.cycles.first() {
                    Some(cycle) => {
                        let mut links = cycle.links.clone();
                        links.sort();
                        match &circle {
                            Some((known, since)) if *known == links && since.elapsed() >= CIRCLE => {
                                fatal(&hub, &format!("tasks waited on each other in a circle: {}", links.join("; ")))
                            }
                            Some((known, _)) if *known == links => {}
                            _ => {
                                hub.log.line(format_args!("tasks wait in a circle: {}", links.join("; ")));
                                circle = Some((links, now));
                            }
                        }
                    }
                    None => circle = None,
                }
            }
        })?;
        Ok(Self { stop, thread: Some(thread) })
    }
}

/// Ends the hub for its service to start it again, the scene and the reason
/// written down first.
fn fatal(hub: &crate::Hub, reason: &str) -> ! {
    fastctx_platform::scene::capture_now(reason);
    hub.log.line(format_args!("{reason}: the hub ends for its service to start it again"));
    hub.log.flush(Duration::from_millis(500));
    eprintln!("fastctx hub: {reason}; exiting for its service to restart it");
    std::process::exit(1)
}

impl Drop for Watchdog {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
