//! New versions of Fastctx on a device that runs no node, a phone
//! (`world-upgrade.md`, 新版本自己往后怎样升级): the owner's choices, a look on
//! GitHub every few hours, what the device tells each hub it is on and hears
//! back, and the application's package brought here, from GitHub or through
//! a hub, checked against its signature. A device fetches nothing for
//! others: it has no network of its own to spare. What it keeps is under
//! `<device>/updates`.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fastctx_types::update::{App, Failed, Fetching, Kind, Looked, Manifest, Report, Settings, State, UpdateCall, Via};
use fastctx_update::Cache;

use super::{Device, failed};
use crate::Error;

#[derive(Default)]
pub(super) struct Updates {
    held: Mutex<Held>,
    started: std::sync::OnceLock<()>,
}

#[derive(Default)]
struct Held {
    settings: Settings,
    looked: Looked,
    /// The release the owner chose to update to, when not the newest.
    chosen: Option<Manifest>,
    app: Option<App>,
    fetching: Option<Fetching>,
    failed: Option<Failed>,
    activity: Option<fastctx_types::update::Stage>,
}

impl Device {
    fn updates_dir(&self) -> PathBuf {
        self.store.root().join("updates")
    }

    fn update_cache(&self) -> Cache {
        Cache::new(self.updates_dir().join("packages"))
    }

    fn keep_update(&self, name: &str, value: &impl serde::Serialize) -> Result<(), Error> {
        let dir = self.updates_dir();
        std::fs::create_dir_all(&dir).map_err(failed)?;
        let bytes = serde_json::to_vec(value).map_err(failed)?;
        fastctx_platform::publish(&dir.join(name), &bytes, fastctx_platform::Permissions::Private).map_err(failed)
    }

    fn kept_update<T: serde::de::DeserializeOwned + Default>(&self, name: &str) -> T {
        std::fs::read(self.updates_dir().join(name))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    /// Reads what was kept and looks on GitHub from now on, once.
    fn start_updates(self: &Arc<Self>) {
        self.updates.started.get_or_init(|| {
            {
                let mut held = self.updates.held.lock().expect("never poisoned");
                held.settings = self.kept_update("settings.json");
                if held.settings.auto {
                    held.settings.chosen = true;
                }
                held.looked = self.kept_update("looked.json");
                held.looked.checking = false;
                held.app = self.kept_update::<Option<App>>("app.json");
            }
            if !fastctx_platform::follows_releases() {
                return;
            }
            let device = Arc::downgrade(self);
            tokio::spawn(async move {
                let settled = tokio::time::Instant::now() + Duration::from_secs(60);
                loop {
                    let Some(device) = device.upgrade() else { return };
                    let due = fastctx_update::github::wait(&device.updates.held.lock().expect("never poisoned").looked)
                        .is_zero();
                    if due && tokio::time::Instant::now() >= settled {
                        device.look_for_versions();
                    }
                    device.report_versions();
                    drop(device);
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
            });
        });
    }

    fn look_for_versions(self: &Arc<Self>) {
        let (running, previous) = {
            let mut held = self.updates.held.lock().expect("never poisoned");
            if held.looked.checking {
                return;
            }
            let previous = held.looked.clone();
            held.looked.checking = true;
            (Self::running(&held), previous)
        };
        self.report_versions();
        let device = self.clone();
        tokio::spawn(async move {
            let looked = fastctx_update::github::look_after(
                Some(&running),
                &previous,
                Some(&device.updates_dir().join("github-list.json")),
            )
            .await;
            {
                let mut held = device.updates.held.lock().expect("never poisoned");
                let mut looked = looked;
                looked.github_at = looked.github_at.max(held.looked.github_at);
                held.looked = looked;
                if let Err(error) = device.keep_update("looked.json", &held.looked) {
                    eprintln!("World device: keeping what was found of new versions: {error}");
                }
            }
            device.report_versions();
        });
    }

    /// What this device tells the hubs it is on of versions now.
    pub(super) fn report_versions(self: &Arc<Self>) {
        self.start_updates();
        if !fastctx_platform::follows_releases() {
            return;
        }
        let state = self.update_state();
        let held = self.updates.held.lock().expect("never poisoned");
        let mut found: Vec<Manifest> =
            [held.looked.stable.clone(), held.looked.beta.clone()].into_iter().flatten().collect();
        found.extend(held.looked.newer.iter().cloned());
        found.extend(self.update_cache().manifests().into_iter().take(1));
        found.sort_by_key(|manifest| std::cmp::Reverse(manifest.semver()));
        found.dedup_by(|a, b| a.version == b.version);
        let report = Report {
            kernel: env!("CARGO_PKG_VERSION").to_string(),
            app: held.app.clone(),
            github: held.looked.github,
            found,
            relays: false,
            holds: Vec::new(),
            asked: None,
            hub_here: false,
            github_speed: None,
            target: None,
            package_progress: false,
            stage: state.stage,
            auto: Some(state.auto),
            auto_chosen: held.settings.chosen,
            kind: Some(Kind::Phone),
            github_at: held.looked.github_at,
            check_failure: held.looked.failure.clone(),
            checked_at: held.looked.at,
            checking: held.looked.checking,
            updates_view: true,
            repair_supported: false,
            repair: None,
        };
        drop(held);
        for session in self.sessions() {
            session.link.report_versions(report.clone());
        }
    }

    /// The version of the application this device runs.
    fn running(held: &Held) -> String {
        held.app.as_ref().map_or_else(|| env!("CARGO_PKG_VERSION").to_string(), |app| app.version.clone())
    }

    /// How updates stand on this device: the line of the application it
    /// runs, and what is offered on it, the release the owner chose while it
    /// is newer, or else the newest. A phone has someone at it: it takes new
    /// releases by itself only when its owner chose so, and never past the
    /// major version of the hubs it is on.
    fn update_state(&self) -> State {
        let sessions = self.sessions();
        let held = self.updates.held.lock().expect("never poisoned");
        let cache = self.update_cache();
        fastctx_update::state::calculate(
            State {
                running: Some(Self::running(&held)),
                settings: held.settings.clone(),
                chosen: held.chosen.clone(),
                looked: held.looked.clone(),
                fetching: held.fetching.clone(),
                failed: held.failed.clone(),
                kept: cache.kept(),
                app: held.app.clone(),
                open: true,
                repair_routing: true,
                ..State::default()
            },
            fastctx_update::state::Context {
                kind: Kind::Phone,
                connected: !sessions.is_empty(),
                hub_here: false,
                networks: sessions.iter().filter_map(|s| s.link.versions()).collect(),
                cached: cache.manifests(),
                activity: held.activity.clone(),
            },
        )
    }

    /// An interface on this device asks about updates.
    pub(super) async fn update(self: &Arc<Self>, call: UpdateCall) -> Result<State, Error> {
        self.start_updates();
        if !fastctx_platform::follows_releases()
            && matches!(
                call,
                UpdateCall::Look | UpdateCall::Fetch { .. } | UpdateCall::Kernel { .. } | UpdateCall::Choose { .. }
            )
        {
            return Err(failed("development builds do not follow releases"));
        }

        match call {
            UpdateCall::Progress { stage } => {
                self.updates.held.lock().expect("never poisoned").activity =
                    (!matches!(stage, fastctx_types::update::Stage::Idle)).then_some(stage);
                self.report_versions();
            }
            UpdateCall::Preferences { auto, .. } => {
                let mut held = self.updates.held.lock().expect("never poisoned");
                let settings = held.settings.clone().with_preferences(auto, Some(false));
                self.keep_update("settings.json", &settings)?;
                held.settings = settings;
                drop(held);
                self.report_versions();
            }
            UpdateCall::State => {}
            UpdateCall::Look => {
                self.look_for_versions();
                while self.updates.held.lock().expect("never poisoned").looked.checking {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
            UpdateCall::Set { mut settings } => {
                settings.relays = false;
                self.keep_update("settings.json", &settings)?;
                self.updates.held.lock().expect("never poisoned").settings = settings;
                self.report_versions();
            }
            UpdateCall::App { app, .. } => {
                let changed = self.updates.held.lock().expect("never poisoned").app.as_ref() != Some(&app);
                if changed {
                    self.keep_update("app.json", &Some(app.clone()))?;
                    {
                        let mut held = self.updates.held.lock().expect("never poisoned");
                        held.app = Some(app);
                        held.activity = None;
                    }
                    self.report_versions();
                }
            }
            UpdateCall::Fetch { version, target } => self.fetch_update(version, target)?,
            UpdateCall::Repair { .. } => return Err(failed("this device does not support local installation repair")),
            UpdateCall::Kernel { .. } => return Err(failed("a device runs no World program of its own")),
            UpdateCall::Choose { version } => {
                let chosen = match version {
                    Some(version) => Some(self.manifest_of(&version).await?),
                    None => None,
                };
                self.updates.held.lock().expect("never poisoned").chosen = chosen;
            }
        }
        Ok(self.update_state())
    }

    /// The manifest of the release `version`: as this device or its hubs
    /// know it, or as GitHub has it.
    async fn manifest_of(&self, version: &str) -> Result<Manifest, Error> {
        let links: Vec<_> = self.sessions().iter().map(|session| session.link.clone()).collect();
        fastctx_update::catalog::manifest(&self.update_state(), self.update_cache().manifests(), &links, version)
            .await
            .map_err(failed)
    }

    fn fetch_update(self: &Arc<Self>, version: String, target: String) -> Result<(), Error> {
        {
            let mut held = self.updates.held.lock().expect("never poisoned");
            if held.fetching.as_ref().is_some_and(|fetching| fetching.version == version && fetching.target == target) {
                return Ok(());
            }
            if held.fetching.is_some() || matches!(held.activity, Some(fastctx_types::update::Stage::Installing { .. }))
            {
                return Err(failed("an update is already being downloaded or installed"));
            }
            held.failed = None;
            held.activity = None;
            held.fetching = Some(Fetching {
                version: version.clone(),
                target: target.clone(),
                via: Via::Github,
                received: 0,
                total: None,
            });
        }
        let device = self.clone();
        tokio::spawn(async move {
            let cache = device.update_cache();
            let sessions = device.sessions();
            let links: Vec<_> = sessions.iter().map(|session| session.link.as_ref()).collect();
            let progressing = device.clone();
            let looked = device.updates.held.lock().expect("never poisoned").looked.clone();
            let network_now = looked.github == Some(false) && looked.github_at <= looked.at;
            let mut progress = move |progress: fastctx_update::Progress| {
                if let Some(fetching) = &mut progressing.updates.held.lock().expect("never poisoned").fetching {
                    fetching.via = progress.via;
                    fetching.received = progress.received;
                    fetching.total = progress.total;
                }
            };
            let outcome = match device.manifest_of(&version).await {
                Ok(manifest) => {
                    fastctx_update::obtain(&cache, &manifest, &target, &links, network_now, &mut progress).await
                }
                Err(why) => Err(why.to_string().into()),
            };
            if outcome.as_ref().is_ok_and(|b| b.github_at.is_some()) {
                let mut held = device.updates.held.lock().expect("never poisoned");
                held.looked.github_at = Some(jiff::Timestamp::now());
                if let Err(error) = device.keep_update("looked.json", &held.looked) {
                    eprintln!("keeping when GitHub was reached: {error}");
                }
            }
            if outcome.is_ok() {
                let held = device.updates.held.lock().expect("never poisoned");
                let mut keep = vec![version.as_str()];
                keep.extend(held.app.as_ref().map(|app| app.version.as_str()));
                keep.extend(held.chosen.as_ref().map(|m| m.version.as_str()));
                keep.extend(held.activity.as_ref().and_then(fastctx_types::update::Stage::version));
                cache.prune(&keep);
            }
            let mut held = device.updates.held.lock().expect("never poisoned");
            held.fetching = None;
            if let Err(why) = outcome {
                held.failed = Some(Failed { version, target, why: why.why, reason: Some(why.reason) });
            }
            drop(held);
            device.report_versions();
        });
        Ok(())
    }
}
