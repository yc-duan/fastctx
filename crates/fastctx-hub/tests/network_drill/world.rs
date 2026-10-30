//! A hub and the machines connected to it, as one scenario has them: the
//! hub runs in a child process the drill can kill and start again, this
//! release's or an earlier one's, and each machine reaches it through a line
//! of its own (`crate::proxy`), so a fault can be put on one machine's line
//! alone. The machines are links in this process, each with its own
//! directory and key, so one can be stopped and started again as itself.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Context as _;
use fastctx_net::link::{Link, Membership, NetDir, Status};
use fastctx_net::login::Device;
use fastctx_net::sign::SecretKey;
use fastctx_types::accounts::MemberKind;
use fastctx_types::{AccountName, HubName, MachineName};

use crate::lab::Lab;
use crate::proxy::Line;

/// The administrator's password on every hub of the drill.
pub const PASSWORD: &str = "Lab-Password-7-quiet-river";

/// Which program serves the hub.
#[derive(Clone)]
pub enum Program {
    /// This release's, served by this very drill in a child process.
    This,
    /// An earlier release's kernel, at this path.
    Earlier(PathBuf),
}

pub struct Hub {
    pub dir: PathBuf,
    pub listen: SocketAddr,
    program: Mutex<Program>,
    /// Where the earlier release keeps what it keeps outside the hub.
    home: PathBuf,
    log: PathBuf,
    child: Mutex<Option<Child>>,
    /// How many times it was killed and started again.
    pub restarts: AtomicUsize,
}

impl Hub {
    /// Makes a hub under `root` that listens at `listen` and gives `public`
    /// as its address, and starts it.
    fn create(root: &Path, program: Program, listen: SocketAddr, public: SocketAddr) -> anyhow::Result<Self> {
        let dir = root.join("hub");
        let home = root.join("hub-home");
        std::fs::create_dir_all(&home)?;
        let admin = AccountName::try_from("root".to_string()).map_err(anyhow::Error::msg)?;
        match &program {
            Program::This => {
                fastctx_hub::init(
                    &dir,
                    fastctx_hub::New {
                        name: HubName::try_from("lab".to_string()).map_err(anyhow::Error::msg)?,
                        public: vec![public],
                        listen,
                        admin,
                        password: PASSWORD.to_string(),
                    },
                )?;
            }
            Program::Earlier(program) => {
                let mut init = earlier(program, &home);
                init.args(["hub", "init", "--name", "lab", "--admin", admin.as_str()])
                    .arg("--public")
                    .arg(public.to_string())
                    .arg("--listen")
                    .arg(listen.to_string())
                    .arg("--dir")
                    .arg(&dir);
                let said = with_password(init)?;
                anyhow::ensure!(said.status.success(), "the earlier release could not make a hub: {}", said.text());
            }
        }
        let hub = Self {
            dir,
            listen,
            program: Mutex::new(program),
            home,
            log: root.join("hub.log"),
            child: Mutex::new(None),
            restarts: AtomicUsize::new(0),
        };
        hub.start()?;
        Ok(hub)
    }

    /// Starts it serving.
    pub fn start(&self) -> anyhow::Result<()> {
        let log = std::fs::OpenOptions::new().create(true).append(true).open(&self.log)?;
        let program = self.program.lock().expect("never poisoned").clone();
        let mut command = match &program {
            Program::This => {
                let mut command = Command::new(std::env::current_exe()?);
                command.arg("hub").arg(&self.dir);
                // Written every second, for the scenarios to read.
                command.env("FASTCTX_HUB_HEALTH_SECONDS", "1");
                command
            }
            Program::Earlier(program) => {
                let mut command = earlier(program, &self.home);
                command.args(["hub", "serve", "--dir"]).arg(&self.dir);
                command
            }
        };
        let mut owned = self.child.lock().expect("never poisoned");
        anyhow::ensure!(owned.is_none(), "the hub already owns a child process");
        *owned = Some(command.stdin(Stdio::null()).stdout(log.try_clone()?).stderr(log).spawn()?);
        Ok(())
    }

    /// The process serving it now; none while it is stopped.
    pub fn pid(&self) -> Option<u32> {
        self.child.lock().expect("never poisoned").as_ref().map(Child::id)
    }

    /// Kills it where it stands, as a power cut does.
    pub fn kill(&self) -> anyhow::Result<()> {
        crate::process::stop(&mut self.child.lock().expect("never poisoned"))
    }

    /// Kills it and starts it again after `after`.
    pub async fn restart(&self, after: Duration) -> anyhow::Result<()> {
        self.kill()?;
        tokio::time::sleep(after).await;
        self.restarts.fetch_add(1, Ordering::SeqCst);
        self.start()
    }

    /// Stops it and serves its directory with this release's hub, as
    /// updating a hub in place does.
    pub fn update(&self) -> anyhow::Result<()> {
        self.kill()?;
        *self.program.lock().expect("never poisoned") = Program::This;
        self.restarts.fetch_add(1, Ordering::SeqCst);
        self.start()
    }

    /// Whether this release's hub serves it now.
    pub fn serves_this(&self) -> bool {
        matches!(*self.program.lock().expect("never poisoned"), Program::This)
    }

    /// What it last wrote of how it is doing; this release's only.
    pub fn health(&self) -> Option<serde_json::Value> {
        serde_json::from_slice(&std::fs::read(self.dir.join("health.json")).ok()?).ok()
    }

    /// What it said in its log, every run of it.
    pub fn log(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }

    /// Its records of where every member's letters stand.
    pub fn letters(&self) -> Option<serde_json::Value> {
        let database = self.dir.join("hub.sqlite3");
        if !database.exists() {
            return serde_json::from_slice(&std::fs::read(self.dir.join("letters.json")).ok()?).ok();
        }
        let c = fastctx_storage::rusqlite::Connection::open_with_flags(
            database,
            fastctx_storage::rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .ok()?;
        let mut stmt = c.prepare("SELECT key,body FROM hub_rows WHERE kind='letters'").ok()?;
        let mut rows = stmt.query([]).ok()?;
        let mut values = serde_json::Map::new();
        while let Some(row) = rows.next().ok()? {
            let key: String = row.get(0).ok()?;
            let bytes: Vec<u8> = row.get(1).ok()?;
            values.insert(key, serde_json::from_slice(&bytes).ok()?);
        }
        Some(serde_json::Value::Object(values))
    }
}

impl Drop for Hub {
    fn drop(&mut self) {
        if let Err(error) = self.kill() {
            crate::process::failed(error);
        }
    }
}

impl Drop for Machine {
    fn drop(&mut self) {
        if let Some(link) = self.link.lock().expect("never poisoned").take() {
            link.stop();
        }
    }
}

pub struct Machine {
    pub name: MachineName,
    pub line: Arc<Line>,
    pub lab: Arc<Lab>,
    link: Mutex<Option<Arc<Link>>>,
    net: PathBuf,
    membership: Membership,
    seed: [u8; 32],
}

impl Machine {
    /// Its link now; the last one while it is stopped.
    pub fn link(&self) -> Arc<Link> {
        self.link.lock().expect("never poisoned").clone().expect("a machine of the drill has a link")
    }

    pub fn stop(&self) {
        let link = self.link();
        if link.failed() {
            crate::process::failed(format!("{}'s essential connection task failed", self.name));
        }
        link.stop();
    }

    /// Starts it again as itself, from what its directory keeps.
    pub fn start(&self) -> anyhow::Result<()> {
        let link = Link::start(
            NetDir::new(self.net.clone()),
            self.membership.clone(),
            SecretKey::from_seed(&self.seed),
            self.lab.clone(),
        )?;
        *self.link.lock().expect("never poisoned") = Some(Arc::new(link));
        Ok(())
    }

    pub fn online(&self) -> bool {
        matches!(self.link().status(), Status::Online { .. })
    }

    /// How many times each of its letters is recorded as one the hub
    /// refused, by the letter's id.
    pub fn refused(&self) -> std::collections::HashMap<String, usize> {
        let mut refused = std::collections::HashMap::new();
        let Ok(entries) = std::fs::read_dir(self.net.join("failures")) else { return refused };
        for entry in entries.flatten() {
            let Ok(bytes) = std::fs::read(entry.path()) else { continue };
            let Ok(failure) = serde_json::from_slice::<serde_json::Value>(&bytes) else { continue };
            if failure["direction"] == "out"
                && let Some(id) = failure["message"]["body"]["id"].as_str()
            {
                *refused.entry(id.to_string()).or_default() += 1;
            }
        }
        refused
    }
}

pub struct World {
    pub root: PathBuf,
    pub account: AccountName,
    pub hub: Hub,
    pub machines: Vec<Machine>,
    /// A line no machine of this process uses: the hub's public address,
    /// for a machine that takes the address it logs in at from the hub.
    pub spare: Arc<Line>,
}

impl World {
    /// A hub served by `program` under `root`, and the machines `names`
    /// logged in to it, each through its own line.
    pub async fn new(root: &Path, program: Program, names: &[&str]) -> anyhow::Result<Self> {
        std::fs::create_dir_all(root)?;
        let account = AccountName::try_from("root".to_string()).map_err(anyhow::Error::msg)?;
        let listen: SocketAddr = format!("127.0.0.1:{}", free_port()?).parse()?;
        let spare = Line::open(listen).await?;
        let hub = Hub::create(root, program, listen, spare.address)?;
        listening(listen, Duration::from_secs(60)).await.context("the hub did not start listening")?;
        let mut machines = Vec::new();
        for name in names {
            let line = Line::open(hub.listen).await?;
            let key = SecretKey::generate()?;
            let machine = MachineName::try_from((*name).to_string()).map_err(anyhow::Error::msg)?;
            let device = Device { name: machine.clone(), key: &key, kind: MemberKind::Computer, register: true };
            let joined = fastctx_net::login::with_password(&[line.address], &account, PASSWORD, device)
                .await
                .map_err(anyhow::Error::msg)
                .with_context(|| format!("logging {name} in"))?;
            let membership = Membership {
                id: joined.hub,
                hub_name: joined.hub_name,
                name: joined.name,
                // Its own line, whatever the hub says its public address is.
                addresses: vec![line.address],
                cert: joined.cert,
                kind: MemberKind::Computer,
                relays: Vec::new(),
            };
            let dir = root.join(name);
            std::fs::create_dir_all(&dir)?;
            let machine = Machine {
                name: machine,
                line,
                lab: Lab::new(&dir.join("link.log"))?,
                link: Mutex::new(None),
                net: dir.join("net"),
                membership,
                seed: *key.seed(),
            };
            machine.start()?;
            machines.push(machine);
        }
        Ok(Self { root: root.to_path_buf(), account, hub, machines, spare })
    }

    pub fn machine(&self, name: &str) -> &Machine {
        self.machines.iter().find(|machine| machine.name.as_str() == name).expect("a machine of the drill")
    }

    /// Waits until every machine is online and sees every other online.
    pub async fn all_online(&self, within: Duration) -> bool {
        let began = Instant::now();
        loop {
            let all = self.machines.iter().all(|machine| {
                machine.online()
                    && self
                        .machines
                        .iter()
                        .all(|other| machine.link().view().member(&other.name).is_some_and(|member| member.online))
            });
            if all {
                return true;
            }
            if began.elapsed() > within {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// How long until `viewer` sees `seen` online, or offline; none
    /// within `within`.
    pub async fn sees(&self, viewer: &str, seen: &MachineName, online: bool, within: Duration) -> Option<Duration> {
        let viewer = self.machine(viewer);
        until(within, || viewer.link().view().member(seen).is_some_and(|member| member.online) == online).await
    }

    /// Stops every machine, then the hub.
    pub async fn end(&self) -> anyhow::Result<()> {
        let mut errors = Vec::new();
        for machine in &self.machines {
            if machine.link().failed() {
                errors.push(format!("{}'s connection task failed", machine.name));
            }
            machine.stop();
        }
        for line in self.machines.iter().map(|machine| &machine.line).chain(std::iter::once(&self.spare)) {
            if let Err(error) = line.end().await {
                errors.push(format!("line shutdown: {error:#}"));
            }
        }
        if let Err(error) = self.hub.kill() {
            errors.push(format!("hub shutdown: {error:#}"));
        }
        if until(Duration::from_secs(10), || self.machines.iter().all(|machine| machine.lab.answering() == 0))
            .await
            .is_none()
        {
            errors.push("business handlers remained after shutdown".to_string());
        }
        anyhow::ensure!(errors.is_empty(), "{}", errors.join("; "));
        Ok(())
    }
}

/// How long until `done` holds, looked at every tenth of a second; none
/// within `within`.
pub async fn until(within: Duration, mut done: impl FnMut() -> bool) -> Option<Duration> {
    let began = Instant::now();
    loop {
        if done() {
            return Some(began.elapsed());
        }
        if began.elapsed() > within {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn listening(at: SocketAddr, within: Duration) -> anyhow::Result<()> {
    let began = Instant::now();
    while tokio::net::TcpStream::connect(at).await.is_err() {
        anyhow::ensure!(began.elapsed() < within, "nothing listens at {at}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Ok(())
}

pub fn free_port() -> std::io::Result<u16> {
    Ok(std::net::TcpListener::bind("127.0.0.1:0")?.local_addr()?.port())
}

/// A command of the earlier release `program`, kept to `home`: its World,
/// its home and its settings are there, it finds no agent's program, and
/// it looks for releases nowhere.
pub fn earlier(program: &Path, home: &Path) -> Command {
    let mut command = Command::new(program);
    for (name, _) in std::env::vars_os() {
        let name = name.to_string_lossy().to_string();
        if name.starts_with("CLAUDE") || name.starts_with("CODEX") || name.starts_with("FASTCTX") {
            command.env_remove(&name);
        }
    }
    command
        .env("FASTCTX_HOME", home)
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_DATA_HOME", home.join(".local").join("share"))
        .env("XDG_STATE_HOME", home.join(".local").join("state"))
        .env("XDG_CACHE_HOME", home.join(".cache"))
        .env("CODEX_HOME", home.join(".codex"))
        .env("CLAUDE_CONFIG_DIR", home.join(".claude"))
        // Only loopback is taken, and nothing answers there.
        .env("FASTCTX_UPDATE_RELEASES", "http://127.0.0.1:9/releases")
        .env("PATH", path_without_agents());
    command
}

/// This process's PATH without the directories that hold an agent's
/// program: no node the drill starts finds Claude Code or Codex.
fn path_without_agents() -> std::ffi::OsString {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let kept: Vec<PathBuf> = std::env::split_paths(&path)
        .filter(|dir| {
            !["claude", "codex", "claude.exe", "codex.exe", "claude.cmd", "codex.cmd"]
                .iter()
                .any(|program| dir.join(program).exists())
        })
        .collect();
    std::env::join_paths(kept).unwrap_or_default()
}

/// What a command printed, and how it ended.
pub struct Said {
    pub status: std::process::ExitStatus,
    pub out: String,
    pub err: String,
}

impl Said {
    pub fn text(&self) -> String {
        format!("{}{}", self.out, self.err).trim().to_string()
    }
}

/// Runs `command` with the drill's password as the first line it reads.
pub fn with_password(command: Command) -> anyhow::Result<Said> {
    crate::process::run(command, Some(format!("{PASSWORD}\n").as_bytes()), Duration::from_secs(60))
}
