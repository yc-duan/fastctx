//! The node's Codex app-servers, one for each Codex home its residents
//! use: an app-server serves the one login of its home, so each account
//! runs its own (`AgentHome`). The default home's starts as before; each
//! other one at its first use, with `CODEX_HOME` naming it and the desktop
//! app's program when its home records none. Whatever any of them reports
//! reaches the node's one listener: threads are told apart by their ids.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use fastctx_hosts::codex::{CodexHost, HostEvent, Log};
use fastctx_types::residents::About;
use tokio::sync::mpsc;

pub(crate) struct CodexHosts {
    default: Arc<CodexHost>,
    others: Mutex<Vec<Arc<CodexHost>>>,
    /// `<world>/codex`, where the default home's app-server runs; each
    /// other one runs in `homes/<key>` under it.
    workdir: PathBuf,
    environment: Vec<(OsString, OsString)>,
    log: Log,
    events: mpsc::UnboundedSender<HostEvent>,
}

impl CodexHosts {
    /// The node's hosts, the default home's `default_home` among them, and
    /// what they all report.
    pub(crate) fn new(
        default_home: PathBuf,
        workdir: PathBuf,
        environment: Vec<(OsString, OsString)>,
        log: Log,
    ) -> (Self, mpsc::UnboundedReceiver<HostEvent>) {
        let (events, merged) = mpsc::unbounded_channel();
        let (default, reports) = CodexHost::new(default_home, None, workdir.clone(), environment.clone(), log.clone());
        forward(reports, events.clone());
        (Self { default, others: Mutex::new(Vec::new()), workdir, environment, log, events }, merged)
    }

    pub(crate) fn default(&self) -> &Arc<CodexHost> {
        &self.default
    }

    /// The host of the home written `dir`; `None` is the default home.
    pub(crate) fn home(&self, dir: Option<&Path>) -> Arc<CodexHost> {
        let Some(dir) = dir.filter(|dir| !crate::homes::same_dir(dir, self.default.config_home())) else {
            return self.default.clone();
        };
        let mut others = self.others.lock().expect("never poisoned");
        if let Some(host) = others.iter().find(|host| crate::homes::same_dir(host.config_home(), dir)) {
            return host.clone();
        }
        let mut environment = self.environment.clone();
        environment.retain(|(name, _)| !name.eq_ignore_ascii_case("CODEX_HOME"));
        environment.push(("CODEX_HOME".into(), dir.as_os_str().to_owned()));
        let workdir = self.workdir.join("homes").join(crate::homes::key(dir));
        let program_home = Some(self.default.config_home().to_path_buf());
        let (host, reports) = CodexHost::new(dir.to_path_buf(), program_home, workdir, environment, self.log.clone());
        forward(reports, self.events.clone());
        others.push(host.clone());
        host
    }

    /// The host a resident's threads are in: its home's.
    pub(crate) fn of(&self, about: &About) -> Arc<CodexHost> {
        self.home(about.home.as_deref())
    }

    /// The host that started or adopted `thread`; the default one when
    /// none did.
    pub(crate) fn thread(&self, thread: &str) -> Arc<CodexHost> {
        if self.default.holds(thread) {
            return self.default.clone();
        }
        let others = self.others.lock().expect("never poisoned");
        others.iter().find(|host| host.holds(thread)).cloned().unwrap_or_else(|| self.default.clone())
    }

    pub(crate) fn all(&self) -> Vec<Arc<CodexHost>> {
        let mut all = vec![self.default.clone()];
        all.extend(self.others.lock().expect("never poisoned").iter().cloned());
        all
    }

    /// Every host has seen each of its turns end and recorded.
    pub(crate) fn settled(&self) -> bool {
        self.all().iter().all(|host| host.settled())
    }

    /// A report's effects are durable: the host it came from lets its turn
    /// go (the others hold none of its threads).
    pub(crate) fn acknowledge(&self, event: &HostEvent) {
        for host in self.all() {
            host.acknowledge(event);
        }
    }
}

fn forward(mut reports: mpsc::UnboundedReceiver<HostEvent>, events: mpsc::UnboundedSender<HostEvent>) {
    tokio::spawn(async move {
        while let Some(event) = reports.recv().await {
            if events.send(event).is_err() {
                return;
            }
        }
    });
}
