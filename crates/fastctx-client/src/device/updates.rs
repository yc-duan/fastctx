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

use fastctx_types::update::{
    App, Channel, Failed, Fetching, Kind, Looked, Manifest, Network, Report, Settings, State, UpdateCall, Via,
};
use fastctx_update::Cache;

use super::{Device, failed};
use crate::Error;

/// How often GitHub is looked at.
const LOOK_EVERY: Duration = Duration::from_secs(6 * 3600);

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
        let temporary = dir.join(format!("{name}.part"));
        std::fs::write(&temporary, bytes).map_err(failed)?;
        std::fs::rename(&temporary, dir.join(name)).map_err(failed)
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
                held.looked = self.kept_update("looked.json");
                held.app = self.kept_update::<Option<App>>("app.json");
            }
            if !fastctx_platform::follows_releases() {
                return;
            }
            let device = Arc::downgrade(self);
            tokio::spawn(async move {
                let since =
                    device.upgrade().and_then(|device| device.updates.held.lock().expect("never poisoned").looked.at);
                let gone = since.map(|at| jiff::Timestamp::now().duration_since(at).unsigned_abs());
                let mut wait =
                    gone.map_or(Duration::ZERO, |gone| LOOK_EVERY.saturating_sub(gone)).max(Duration::from_secs(60));
                loop {
                    tokio::time::sleep(wait).await;
                    let Some(device) = device.upgrade() else { return };
                    device.look_for_versions().await;
                    wait = LOOK_EVERY;
                }
            });
        });
    }

    async fn look_for_versions(self: &Arc<Self>) {
        let running = self.updates.held.lock().expect("never poisoned").app.as_ref().map(|app| app.version.clone());
        let looked = fastctx_update::github::look(Some(running.as_deref().unwrap_or(env!("CARGO_PKG_VERSION")))).await;
        {
            let mut held = self.updates.held.lock().expect("never poisoned");
            if looked.why.is_none() {
                held.looked = looked;
            } else {
                held.looked.at = looked.at;
                held.looked.github = looked.github;
                held.looked.why = looked.why;
            }
            let looked = held.looked.clone();
            drop(held);
            if let Err(error) = self.keep_update("looked.json", &looked) {
                eprintln!("World device: keeping what was found of new versions: {error}");
            }
        }
        self.report_versions();
    }

    /// What this device tells the hubs it is on of versions now.
    pub(super) fn report_versions(self: &Arc<Self>) {
        self.start_updates();
        if !fastctx_platform::follows_releases() {
            return;
        }
        let held = self.updates.held.lock().expect("never poisoned");
        let mut found: Vec<Manifest> =
            [held.looked.stable.clone(), held.looked.beta.clone()].into_iter().flatten().collect();
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
        };
        drop(held);
        for session in self.sessions() {
            session.link.report_versions(report.clone());
        }
    }

    fn network(&self) -> Option<Network> {
        let mut merged: Option<Network> = None;
        for network in self.sessions().iter().filter_map(|session| session.link.versions()) {
            let Some(kept) = &mut merged else {
                merged = Some(network);
                continue;
            };
            kept.stable = fastctx_types::update::newest(kept.stable.as_ref(), network.stable.as_ref()).cloned();
            kept.beta = fastctx_types::update::newest(kept.beta.as_ref(), network.beta.as_ref()).cloned();
            kept.policy.minimum = match (kept.policy.minimum.take(), network.policy.minimum) {
                (Some(a), Some(b)) => {
                    Some(if fastctx_types::update::release(&b) > fastctx_types::update::release(&a) { b } else { a })
                }
                (a, b) => a.or(b),
            };
            kept.hub = match (kept.hub.take(), network.hub) {
                (Some(a), Some(b)) => {
                    Some(if fastctx_types::update::release(&b) < fastctx_types::update::release(&a) { b } else { a })
                }
                _ => None,
            };
            kept.machines.extend(network.machines);
            kept.due.extend(network.due);
        }
        merged
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
        let network = self.network();
        let held = self.updates.held.lock().expect("never poisoned");
        let running = Self::running(&held);
        let channel = Channel::of(&running);
        let cache = self.update_cache();
        let kept = cache.manifests();
        let newest = [held.looked.on(channel), network.as_ref().and_then(|network| network.on(channel))]
            .into_iter()
            .chain(kept.iter().filter(|manifest| channel.takes(&manifest.version)).map(Some))
            .fold(None, fastctx_types::update::newest)
            .cloned();
        let chosen = held.chosen.clone().filter(|chosen| chosen.newer_than(&running));
        let offer = chosen.clone().or(newest);
        let auto = held.settings.auto_for(Kind::App, network.as_ref().map(|network| &network.policy));
        let hub_major = network
            .as_ref()
            .and_then(|network| network.hub.as_deref())
            .and_then(fastctx_types::update::release)
            .map(|version| version.major);
        State {
            running: Some(running.clone()),
            settings: held.settings.clone(),
            channel,
            auto,
            chosen,
            hub_major,
            looked: held.looked.clone(),
            network,
            offer,
            fetching: held.fetching.clone(),
            failed: held.failed.clone(),
            kept: cache.kept(),
            app: held.app.clone(),
            kernel_target: String::new(),
            renewing: None,
            take: None,
            open: true,
        }
    }

    /// An interface on this device asks about updates.
    pub(super) async fn update(self: &Arc<Self>, call: UpdateCall) -> Result<State, Error> {
        self.start_updates();
        match call {
            UpdateCall::State => {}
            UpdateCall::Look => self.look_for_versions().await,
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
                    self.updates.held.lock().expect("never poisoned").app = Some(app);
                    self.report_versions();
                }
            }
            UpdateCall::Fetch { version, target } => self.fetch_update(version, target)?,
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
        let state = self.update_state();
        let known =
            [state.offer.as_ref(), state.chosen.as_ref(), state.looked.stable.as_ref(), state.looked.beta.as_ref()]
                .into_iter()
                .flatten()
                .chain(state.looked.newer.iter())
                .chain(
                    state
                        .network
                        .as_ref()
                        .into_iter()
                        .flat_map(|network| network.stable.iter().chain(network.beta.iter())),
                )
                .find(|manifest| manifest.version == version)
                .cloned()
                .or_else(|| self.update_cache().manifests().into_iter().find(|manifest| manifest.version == version));
        match known {
            Some(manifest) => Ok(manifest),
            None => fastctx_update::github::manifest_of(&state.looked.releases, version).await.map_err(failed),
        }
    }

    fn fetch_update(self: &Arc<Self>, version: String, target: String) -> Result<(), Error> {
        let state = self.update_state();
        let manifest =
            [state.offer.as_ref(), state.chosen.as_ref(), state.looked.stable.as_ref(), state.looked.beta.as_ref()]
                .into_iter()
                .flatten()
                .chain(state.looked.newer.iter())
                .chain(
                    state
                        .network
                        .as_ref()
                        .into_iter()
                        .flat_map(|network| network.stable.iter().chain(network.beta.iter())),
                )
                .find(|manifest| manifest.version == version)
                .cloned()
                .ok_or_else(|| failed(format!("no release {version} is known here")))?;
        {
            let mut held = self.updates.held.lock().expect("never poisoned");
            if held.fetching.as_ref().is_some_and(|fetching| fetching.version == version && fetching.target == target) {
                return Ok(());
            }
            held.failed = None;
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
            let link = sessions.iter().map(|session| session.link.as_ref()).find(|link| link.keeps_versions());
            let progressing = device.clone();
            let mut progress = move |progress: fastctx_update::Progress| {
                if let Some(fetching) = &mut progressing.updates.held.lock().expect("never poisoned").fetching {
                    fetching.via = progress.via;
                    fetching.received = progress.received;
                    fetching.total = progress.total;
                }
            };
            let outcome = fastctx_update::obtain(&cache, &manifest, &target, link, &mut progress).await;
            if outcome.is_ok() {
                cache.prune(&[manifest.version.as_str()]);
            }
            let mut held = device.updates.held.lock().expect("never poisoned");
            held.fetching = None;
            if let Err(why) = outcome {
                held.failed = Some(Failed { version, target, why });
            }
            drop(held);
            device.report_versions();
        });
        Ok(())
    }
}
