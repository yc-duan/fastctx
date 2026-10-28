//! The node's Codex app-servers, one for each Codex login and a separate
//! instruction and extension home for its assisted conversations. An
//! app-server serves one login, so each account
//! runs its own (`AgentHome`). The default home's starts as before; each
//! other one at its first use, with `CODEX_HOME` naming it and the desktop
//! app's program when its home records none. Whatever any of them reports
//! reaches the node's one listener: threads are told apart by their ids;
//! what they read of their logins' limits, and what each app-server says of
//! its login on starting, reach the logins' record (`crate::logins`).

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use fastctx_hosts::codex::identify::SaidReport;
use fastctx_hosts::codex::launch::{self, Launch};
use fastctx_hosts::codex::{CodexHost, HostEvent, LimitsReport, Log};
use fastctx_types::ipc::Failure;
use fastctx_types::residents::About;
use tokio::sync::mpsc;

pub(crate) struct CodexHosts {
    default: Arc<CodexHost>,
    others: Mutex<Vec<Arc<CodexHost>>>,
    assisted: Mutex<Vec<Arc<CodexHost>>>,
    /// `<world>/codex`, where the default home's app-server runs; each
    /// other one runs in `homes/<key>` under it.
    workdir: PathBuf,
    environment: Vec<(OsString, OsString)>,
    log: Log,
    events: mpsc::UnboundedSender<HostEvent>,
    /// What every host reads or hears of its login's limits.
    limits: mpsc::UnboundedSender<LimitsReport>,
    /// What every host's app-server says of its login on starting.
    said: mpsc::UnboundedSender<SaidReport>,
}

/// What the node's hosts tell, each its own way: their threads' reports,
/// what they read or hear of their logins' limits, and what their
/// app-servers say of their logins on starting.
pub(crate) struct Heard {
    pub events: mpsc::UnboundedReceiver<HostEvent>,
    pub limits: mpsc::UnboundedReceiver<LimitsReport>,
    pub said: mpsc::UnboundedReceiver<SaidReport>,
}

impl CodexHosts {
    /// The node's hosts, the default home's `default_home` among them, and
    /// what they all tell.
    pub(crate) fn new(
        default_home: PathBuf,
        workdir: PathBuf,
        environment: Vec<(OsString, OsString)>,
        log: Log,
    ) -> (Self, Heard) {
        let (events, merged) = mpsc::unbounded_channel();
        let (limits, limits_heard) = mpsc::unbounded_channel();
        let (said, said_heard) = mpsc::unbounded_channel();
        let (default, reports) = CodexHost::new(default_home, None, workdir.clone(), environment.clone(), log.clone());
        default.report_limits_to(limits.clone());
        default.report_said_to(said.clone());
        forward(reports, events.clone());
        let hosts = Self {
            default,
            others: Mutex::new(Vec::new()),
            assisted: Mutex::new(Vec::new()),
            workdir,
            environment,
            log,
            events,
            limits,
            said,
        };
        (hosts, Heard { events: merged, limits: limits_heard, said: said_heard })
    }

    pub(crate) fn default(&self) -> &Arc<CodexHost> {
        &self.default
    }

    /// The host of the home written `dir`; `None` is the default home. A
    /// home whose own working directory cannot be kept fails what needed
    /// it, and only that.
    pub(crate) fn home(&self, dir: Option<&Path>) -> Result<Arc<CodexHost>, Failure> {
        let Some(dir) = dir.filter(|dir| !crate::homes::same_dir(dir, self.default.config_home())) else {
            return Ok(self.default.clone());
        };
        let mut others = self.others.lock().expect("never poisoned");
        if let Some(host) = others.iter().find(|host| crate::homes::same_dir(host.config_home(), dir)) {
            return Ok(host.clone());
        }
        let mut environment = self.environment.clone();
        environment.retain(|(name, _)| !name.eq_ignore_ascii_case("CODEX_HOME"));
        environment.push(("CODEX_HOME".into(), dir.as_os_str().to_owned()));
        let workdir = super::directories::get(&self.workdir.join("homes"), dir).map_err(|error| {
            Failure::new(format!("keeping the Codex home {} apart from the others: {error}", dir.display()))
        })?;
        let program_home = Some(self.default.config_home().to_path_buf());
        let (host, reports) = CodexHost::new(dir.to_path_buf(), program_home, workdir, environment, self.log.clone());
        host.report_limits_to(self.limits.clone());
        host.report_said_to(self.said.clone());
        forward(reports, self.events.clone());
        others.push(host.clone());
        Ok(host)
    }

    /// The host of the home written `dir` when the node holds one already;
    /// none is made for the asking.
    pub(crate) fn existing(&self, dir: &Path) -> Option<Arc<CodexHost>> {
        if crate::homes::same_dir(dir, self.default.config_home()) {
            return Some(self.default.clone());
        }
        self.others
            .lock()
            .expect("never poisoned")
            .iter()
            .find(|host| crate::homes::same_dir(host.config_home(), dir))
            .cloned()
    }

    /// The Codex program the host of the home written `dir` starts, as it
    /// would start it now (`launch::discover_for`): read from the homes'
    /// configurations and the search path, with nothing made or started for
    /// the asking.
    pub(crate) fn launch(&self, dir: &Path) -> Result<Option<Launch>, String> {
        if let Some(host) = self.existing(dir) {
            return host.launch();
        }
        // As `home` would make its host: the default home's desktop app
        // program, the environment's search path.
        let path = self.environment.iter().find(|(name, _)| name.eq_ignore_ascii_case("PATH")).map(|(_, value)| value);
        launch::discover_for(dir, Some(self.default.config_home()), path)
    }

    /// The environment every app-server runs with, before its home is named.
    pub(crate) fn environment(&self) -> &[(OsString, OsString)] {
        &self.environment
    }

    pub(crate) fn log(&self) -> Log {
        self.log.clone()
    }

    /// Where an app-server started only to ask a login who it is runs:
    /// apart from every host's, so it never shares their sockets.
    pub(crate) fn asking_dir(&self, dir: &Path) -> Result<PathBuf, Failure> {
        super::directories::get(&self.workdir.join("identify"), dir).map_err(|error| {
            Failure::new(format!("keeping a directory to ask the Codex home {} in: {error}", dir.display()))
        })
    }

    /// The original or isolated host recorded when the conversation was born.
    /// The app-server that serves the thread of `about`. The World's
    /// app-servers serve only the threads it holds: a thread the user opened
    /// on their own is never loaded, read or controlled there
    /// (`world-layers.md`).
    pub(crate) fn of(&self, node: &crate::Node, about: &About) -> Result<Arc<CodexHost>, Failure> {
        if !about.world_managed() {
            return Err(Failure::refused(crate::speech::OWN_SESSION));
        }
        self.session(about.home.as_deref(), crate::assists::host::kept(node, &about.id)?.unwrap_or(false))
    }

    pub(crate) fn session(&self, dir: Option<&Path>, assisting: bool) -> Result<Arc<CodexHost>, Failure> {
        let owner = self.home(dir)?;
        if !assisting {
            return Ok(owner);
        }
        let mut assisted = self.assisted.lock().expect("never poisoned");
        if let Some(host) = assisted.iter().find(|host| crate::homes::same_dir(host.config_home(), owner.config_home()))
        {
            return Ok(host.clone());
        }
        let workdir = super::directories::get(&self.workdir.join("assisted"), owner.config_home())
            .map_err(|error| Failure::new(format!("keeping the assisted Codex home: {error}")))?;
        let (host, reports) = CodexHost::new_assisted(owner, workdir).map_err(Failure::new)?;
        host.report_limits_to(self.limits.clone());
        forward(reports, self.events.clone());
        assisted.push(host.clone());
        Ok(host)
    }

    pub(crate) fn all(&self) -> Vec<Arc<CodexHost>> {
        let mut all = vec![self.default.clone()];
        all.extend(self.others.lock().expect("never poisoned").iter().cloned());
        all.extend(self.assisted.lock().expect("never poisoned").iter().cloned());
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
