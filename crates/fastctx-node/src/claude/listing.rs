//! Startup observers share one CLI result per login. Supervisor and session
//! files invalidate it; process exits and new launches require fresh evidence.
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, Instant},
};
use tokio::sync::watch;

pub(super) type Listings = crate::leases::Registry<(PathBuf, Option<OsString>), Result<Listing, String>>;

pub(super) struct Listing {
    listed: fastctx_platform::doing::Mutex<Option<Listed>>,
    changed: watch::Receiver<u64>,
    /// None where Claude Code's records could not be watched: a list is then
    /// kept no longer than one look, as before anything was watched.
    _watcher: Option<Mutex<fastctx_watch::Watcher>>,
}
struct Listed {
    at: Instant,
    revision: u64,
    failures: u32,
    answer: Result<String, String>,
}

// File observation is the ordinary trigger. A periodic read also reconciles
// a supervisor that changed its facts without changing its recorded state.
const RECONCILE: Duration = Duration::from_secs(300);

impl Listing {
    pub(super) fn new(command: &super::Command, login: &str) -> Result<Self, String> {
        let home = command
            .env
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case("CLAUDE_CONFIG_DIR"))
            .map(|(_, value)| PathBuf::from(value))
            .map(Ok)
            .unwrap_or_else(|| fastctx_platform::user_home().map(|home| home.join(".claude")))
            .map_err(|error| error.to_string())?;
        let home = crate::file_events::named(&home).map_err(|error| error.to_string())?;
        let (changed, receiver) = watch::channel(0u64);
        let watched = home.clone();
        let mut watcher = fastctx_watch::Watcher::new(move |event: notify::Result<notify::Event>| {
            let relevant = match event {
                Err(_) => true,
                Ok(event) => {
                    event.need_rescan()
                        || crate::file_events::changes(&event)
                            && event.paths.iter().any(|path| path.strip_prefix(&watched).is_ok_and(supervisor_event))
                }
            };
            if relevant {
                changed
                    .send_modify(|revision| *revision = revision.checked_add(1).expect("listing revision exhausted"));
            }
        })
        .following_links(false);
        let watcher = watcher
            .watch_within(&home, |relative| supervisor_path(relative) && relative.components().count() <= 2)
            .ok()
            .map(|()| Mutex::new(watcher));
        Ok(Self { listed: crate::ranked::listing(&login), changed: receiver, _watcher: watcher })
    }

    /// Each change to the supervisor's records, for an observer to look again
    /// as soon as one comes rather than at its next turn.
    pub(super) fn changes(&self) -> watch::Receiver<u64> {
        self.changed.clone()
    }

    pub(super) async fn agents(&self, command: &super::Command, after: Option<Instant>) -> Result<String, String> {
        let mut listing = self.listed.lock().await;
        let revision = *self.changed.borrow();
        if let Some(listed) = &*listing {
            let lifetime = if self._watcher.is_none() {
                fastctx_hosts::claude::MOD_LOOK
            } else if listed.answer.is_ok() {
                RECONCILE
            } else {
                Duration::from_secs(2u64.saturating_pow(listed.failures.min(6)).min(60))
            };
            if listed.revision == revision
                && after.is_none_or(|after| listed.at >= after)
                && listed.at.elapsed() < lifetime
            {
                return listed.answer.clone();
            }
        }
        let failures = listing.as_ref().map_or(0, |listed| listed.failures);
        let at = Instant::now();
        let answer = super::startup::run(command, vec!["agents".into(), "--json".into(), "--all".into()])
            .await
            .and_then(|text| {
                serde_json::from_str::<Vec<serde_json::Value>>(&text)
                    .map(|_| text)
                    .map_err(|error| format!("reading `claude agents --json --all`: {error}"))
            });
        let failures = if answer.is_ok() { 0 } else { failures.saturating_add(1) };
        *listing = Some(Listed { at, revision, failures, answer: answer.clone() });
        answer
    }
}

fn supervisor_path(relative: &Path) -> bool {
    relative.components().next().is_some_and(|part| part.as_os_str() == "jobs" || part.as_os_str() == "sessions")
}

fn supervisor_event(relative: &Path) -> bool {
    let mut parts = relative.components();
    let Some(first) = parts.next() else { return true };
    match first.as_os_str().to_str() {
        Some("sessions") => true,
        Some("jobs") => {
            let rest: Vec<_> = parts.collect();
            rest.len() <= 1 || rest.len() == 2 && rest[1].as_os_str() == "state.json"
        }
        _ => false,
    }
}
