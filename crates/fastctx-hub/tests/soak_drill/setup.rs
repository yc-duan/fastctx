//! The drill's network: its hub, the lines to it that faults act on, the
//! node under test in a slice of its own, the patrol machine, and the
//! accounts' devices.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use fastctx_client::World;
use fastctx_client::device::Device;
use fastctx_net::link::{Membership, NetDir};
use fastctx_net::login;
use fastctx_net::sign::SecretKey;
use fastctx_types::accounts::{AccountChange, MemberKind};
use fastctx_types::ipc::Peer;
use fastctx_types::layout::WorldDir;
use fastctx_types::{AccountName, AgentName, MachineName};
use serde_json::json;

use crate::faults::{Faults, say};
use crate::judge::{Marks, mark};
use crate::proxy::Line;

pub(crate) const PASS: &str = "Soaking-Through-88!";
/// The machine under test, and the one its patrol runs on.
pub(crate) const SOAK: &str = "alice-soak";
pub(crate) const PATROL: &str = "alice-patrol";

/// What `scripts/soak.py` hands the drill.
#[derive(Clone, Debug, serde::Deserialize)]
pub(crate) struct Config {
    /// Where the drill keeps its hub, the patrol machine, the devices and
    /// what it found.
    pub base: PathBuf,
    /// The home of the node under test's user, on a disk of its own, and
    /// its World where a release keeps it there: whatever its Codex starts
    /// of the World's finds this World, as on a server.
    pub home: PathBuf,
    pub world: PathBuf,
    /// Codex's home for the node under test, on the same disk.
    pub codex_home: PathBuf,
    pub kernel: PathBuf,
    /// The PATH the World's processes see: Codex on it, Claude not.
    pub path: String,
    /// How long the load runs.
    pub minutes: u64,
    /// Conversations opened before the load, the turns each is given, and
    /// the minutes they are given in: a node that takes them slower is
    /// measured all the same, with what it took.
    pub conversations: usize,
    pub turns: usize,
    pub seeding: u64,
    /// People at this machine's interface, typing, sending and stopping.
    pub workers: usize,
    /// This machine's interfaces watching it.
    pub watchers: usize,
    /// Phones watching and asking from afar.
    pub devices: usize,
    /// The slice the node under test runs in, and the disk its World is on:
    /// its device, which systemd's limits take (it takes no `MAJOR:MINOR`,
    /// and says nothing of it), and its number, which the cgroup's own
    /// limits name it by.
    pub slice: String,
    pub disk: String,
    pub number: String,
    /// The disk's operations a second as a small server's is.
    pub iops: u64,
}

pub(crate) fn account(name: &str) -> AccountName {
    name.parse().expect("an account name")
}

pub(crate) fn machine(name: &str) -> MachineName {
    name.parse().expect("a machine name")
}

pub(crate) async fn pause() {
    tokio::time::sleep(Duration::from_millis(250)).await;
}

/// A free port on this machine's loopback.
fn free() -> Result<SocketAddr> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    Ok(listener.local_addr()?)
}

/// `name` as found on this process's PATH.
fn found(name: &str) -> Result<PathBuf> {
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
        .with_context(|| format!("{name} is not on the PATH"))
}

/// A node's process environment, its user's home `home`: nothing of this
/// process's own.
fn command(config: &Config, root: &Path, program: &Path, home: &Path) -> Command {
    let mut command = Command::new(program);
    command
        .env_clear()
        .env("PATH", &config.path)
        .env("HOME", home)
        .env("LANG", "C.UTF-8")
        .env("FASTCTX_HOME", root)
        .env("CODEX_HOME", &config.codex_home)
        .env("CLAUDE_CONFIG_DIR", config.base.join("claude"))
        .env("FASTCTX_UPDATE_RELEASES", "http://127.0.0.1:9/releases")
        .current_dir(config.base.join("user"))
        .stdin(Stdio::null());
    command
}

fn output(path: &Path) -> Result<(Stdio, Stdio)> {
    let file = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    Ok((Stdio::from(file.try_clone()?), Stdio::from(file)))
}

/// Joins `root` to the hub at `listen` as `label` of `owner`, its node to
/// reach the hub at `through`, with the kernel placed where its World runs
/// it from.
async fn join(
    config: &Config,
    root: &Path,
    label: &str,
    owner: &str,
    listen: SocketAddr,
    through: SocketAddr,
) -> Result<()> {
    let dir = WorldDir::under(root);
    std::fs::create_dir_all(dir.path())?;
    // Its World says a service keeps its node, so a call while it is down
    // waits for the drill to start it again, in its slice, rather than
    // starting one of its own outside it.
    std::fs::write(dir.node_service(), b"")?;
    let key = SecretKey::generate()?;
    let joined = login::with_password(
        &[listen],
        &account(owner),
        PASS,
        login::Device { name: machine(label), key: &key, kind: MemberKind::Computer, register: true },
    )
    .await
    .map_err(anyhow::Error::msg)?;
    key.create(&dir.device_key())?;
    std::fs::write(dir.machine_file(), serde_json::to_vec(&json!({"name": label, "was": []}))?)?;
    let net = NetDir::new(dir.net().join(joined.hub.as_str()));
    net.prepare()?;
    let membership = Membership {
        id: joined.hub,
        hub_name: joined.hub_name,
        name: joined.name,
        addresses: vec![through],
        cert: joined.cert,
        kind: MemberKind::Computer,
        relays: joined.relays,
    };
    fastctx_net::link::state::write(&net.membership(), &membership)?;
    std::fs::create_dir_all(root.join("bin"))?;
    let installed = root.join("bin/fastctx");
    if std::fs::hard_link(&config.kernel, &installed).is_err() {
        std::fs::copy(&config.kernel, &installed)?;
    }
    Ok(())
}

/// A file of the cgroup of `slice`, whose name's dashes systemd takes for
/// the slices it is in.
fn cgroup(slice: &str, file: &str) -> String {
    let name = slice.trim_end_matches(".slice");
    let mut path = PathBuf::from("/sys/fs/cgroup");
    let parts: Vec<&str> = name.split('-').collect();
    for end in 1..=parts.len() {
        path.push(format!("{}.slice", parts[..end].join("-")));
    }
    std::fs::read_to_string(path.join(file)).map_or_else(|error| format!("({error})"), |text| text.trim().to_string())
}

/// Whether the node at `root` answers within `within`; with `online`,
/// also that it is on its hub.
pub(crate) async fn answers(root: &Path, within: Duration, online: bool) -> bool {
    let world = World::new(WorldDir::under(root), Peer::Command { resident: None, session: None });
    let began = Instant::now();
    while began.elapsed() < within {
        if let Ok(Ok(nodes)) = tokio::time::timeout(Duration::from_secs(2), world.nodes()).await
            && (!online || serde_json::to_string(&nodes).is_ok_and(|text| text.contains("online")))
        {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

/// The node under test: its World on the disk of its own, its process in
/// the slice whose limits are a small server's, kept as its service keeps
/// it on a server: started again at once whenever it ends but by the
/// drill's own hand.
pub(crate) struct Soak {
    pub config: Config,
    systemd_run: PathBuf,
    /// The node's process. The drill takes it out before ending it, so one
    /// that `watch` finds ended here ended on its own.
    child: Arc<Mutex<Option<Child>>>,
    pid: Arc<AtomicU32>,
    ended: Arc<Mutex<Vec<Ended>>>,
}

/// The node ending on its own: when, what its log and its slice said of
/// it, and how long its next process took to take requests.
#[derive(Clone, Debug, serde::Serialize)]
pub(crate) struct Ended {
    pub at: jiff::Timestamp,
    pub said: Vec<String>,
    pub memory_events: String,
    pub back_after_seconds: Option<f64>,
}

/// Starts the node in its slice unless it has a process, running or yet
/// to be found ended: `systemd-run --scope` registers the scope, then
/// becomes the node, so the child is the node itself.
fn start(config: &Config, systemd_run: &Path, child: &Mutex<Option<Child>>, pid: &AtomicU32) -> Result<()> {
    let mut slot = child.lock().expect("never poisoned");
    if slot.is_some() {
        return Ok(());
    }
    let (out, err) = output(&config.base.join("soak-node.out"))?;
    let mut command = command(config, &config.world, systemd_run, &config.home);
    command
        .args(["--scope", "--quiet", "--collect"])
        .arg(format!("--slice={}", config.slice))
        .arg("--")
        .arg(config.world.join("bin/fastctx"))
        .arg("node")
        .stdout(out)
        .stderr(err);
    let started = command.spawn().context("starting the node under test in its slice")?;
    pid.store(started.id(), Ordering::Release);
    *slot = Some(started);
    Ok(())
}

impl Soak {
    pub fn root(&self) -> &Path {
        &self.config.world
    }

    /// The node's process now, while it has one.
    pub fn pid(&self) -> Option<u32> {
        Some(self.pid.load(Ordering::Acquire)).filter(|pid| *pid != 0)
    }

    /// The node's process as it changes.
    pub fn pids(&self) -> Arc<AtomicU32> {
        self.pid.clone()
    }

    pub fn start(&self) -> Result<()> {
        start(&self.config, &self.systemd_run, &self.child, &self.pid)
    }

    /// Ends the node at once, as a power cut or `kill -9` does.
    pub fn kill(&self) {
        let child = self.child.lock().expect("never poisoned").take();
        self.pid.store(0, Ordering::Release);
        if let Some(mut child) = child {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    /// The times the node ended on its own.
    pub fn ended(&self) -> Vec<Ended> {
        self.ended.lock().expect("never poisoned").clone()
    }

    /// Starts the node again at once whenever it ends on its own, as its
    /// service (`Restart=always`, no delay) does on a server, and tells of
    /// it: the end among the faults, what the node and its slice said, and
    /// once it takes requests again, a mark of its new process's readings,
    /// which count from its start.
    pub fn watch(&self, faults: Arc<Faults>, marks: Arc<Marks>) {
        let (config, systemd_run) = (self.config.clone(), self.systemd_run.clone());
        let (child, pid, ended) = (self.child.clone(), self.pid.clone(), self.ended.clone());
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(500)).await;
                let gone = {
                    let mut slot = child.lock().expect("never poisoned");
                    match slot.as_mut().map(Child::try_wait) {
                        Some(Ok(Some(_))) => slot.take(),
                        _ => None,
                    }
                };
                if gone.is_none() {
                    continue;
                }
                pid.store(0, Ordering::Release);
                let index = faults.begin("ended");
                let at = jiff::Timestamp::now();
                let log = std::fs::read_to_string(config.world.join("world/node.log")).unwrap_or_default();
                let mut said: Vec<String> = log.lines().rev().take(40).map(str::to_string).collect();
                said.reverse();
                let memory_events = cgroup(&config.slice, "memory.events");
                println!("FAIL the node under test ended on its own; its slice: {memory_events}");
                for line in &said {
                    println!("  {line}");
                }
                let began = Instant::now();
                while let Err(error) = start(&config, &systemd_run, &child, &pid) {
                    say(&format!("soak: starting the node again: {error:#}"));
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
                let back = answers(&config.world, Duration::from_secs(120), false).await;
                let back_after_seconds = back.then(|| began.elapsed().as_secs_f64());
                faults.end(index);
                let count = {
                    let mut ended = ended.lock().expect("never poisoned");
                    ended.push(Ended { at, said, memory_events, back_after_seconds });
                    ended.len()
                };
                if back {
                    mark(&config.world, &pid, &marks, &format!("back-{count}")).await;
                }
            }
        });
    }

    /// What the node's slice holds it to, and what it came to.
    pub fn held(&self) -> serde_json::Value {
        let slice = &self.config.slice;
        json!({
            "cpus": cgroup(slice, "cpuset.cpus.effective"),
            "memory_max": cgroup(slice, "memory.max"),
            "memory_peak": cgroup(slice, "memory.peak"),
            "memory_events": cgroup(slice, "memory.events"),
            "io_max": cgroup(slice, "io.max"),
        })
    }

    /// Sets the slice's limit on the disk's operations a second, and reads
    /// it back from the cgroup: systemd says it set a limit it did not.
    pub fn disk(&self, iops: u64) -> Result<()> {
        let device = &self.config.disk;
        let status = Command::new("systemctl")
            .args(["set-property", "--runtime", &self.config.slice])
            .arg(format!("IOReadIOPSMax={device} {iops}"))
            .arg(format!("IOWriteIOPSMax={device} {iops}"))
            .status()?;
        anyhow::ensure!(status.success(), "setting the disk's limit to {iops} operations a second");
        let set = cgroup(&self.config.slice, "io.max");
        anyhow::ensure!(
            set.lines().any(|line| {
                line.starts_with(&format!("{} ", self.config.number))
                    && line.contains(&format!("riops={iops} "))
                    && line.ends_with(&format!("wiops={iops}"))
            }),
            "the disk's limit of {iops} operations a second is not in the slice's io.max: {set:?}"
        );
        Ok(())
    }

    /// Holds the slice to a small server's two cores and two gigabytes.
    pub fn hold(&self, cpus: &str) -> Result<()> {
        let status = Command::new("systemctl")
            .args(["set-property", "--runtime", &self.config.slice])
            .arg(format!("AllowedCPUs={cpus}"))
            .args(["MemoryMax=2G", "MemorySwapMax=0"])
            .status()?;
        anyhow::ensure!(status.success(), "holding the slice to two cores and two gigabytes");
        Ok(())
    }
}

impl Drop for Soak {
    fn drop(&mut self) {
        self.kill();
    }
}

/// The hub as its own process.
pub(crate) struct Hub {
    pub dir: PathBuf,
    child: Child,
}

impl Hub {
    pub fn log(&self) -> String {
        std::fs::read_to_string(self.dir.join("hub.log")).unwrap_or_default()
    }
}

impl Drop for Hub {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Everything the drill set up.
pub(crate) struct Lab {
    pub config: Config,
    pub hub: Hub,
    /// The node under test's line to the hub, and the devices'.
    pub node_line: Arc<Line>,
    pub device_line: Arc<Line>,
    pub soak: Soak,
    pub patrol: Child,
    pub patrol_root: PathBuf,
    /// This machine's interface, as its user.
    pub local: World,
    /// The owner's own phone, on a line of its own: what alerts reach.
    pub owner: World,
    pub _owner_device: Arc<Device>,
    /// The phones, by directory and account, logged in once.
    pub phones: Vec<(PathBuf, AccountName)>,
    pub agent: AgentName,
}

impl Drop for Lab {
    fn drop(&mut self) {
        let _ = self.patrol.kill();
        let _ = self.patrol.wait();
    }
}

/// A phone at `directory` logged in to `owner` at `listen`.
pub(crate) async fn phone(directory: &Path, owner: &str, listen: SocketAddr) -> Result<(Arc<Device>, World)> {
    let cache = fastctx_client::cache::Cache::open(directory.join("cache"))?;
    let device = Device::open_managed(directory.join("device"), cache.clone())?;
    let name = directory.file_name().and_then(|name| name.to_str()).context("a phone's name")?;
    device.login(vec![listen], account(owner), PASS.into(), Some(machine(name))).await?;
    for _ in 0..240 {
        if device.state().hubs.iter().any(|hub| hub.logins.contains(&account(owner))) {
            return Ok((device.clone(), World::from_device(device).keeping(cache)));
        }
        pause().await;
    }
    anyhow::bail!("the phone {name} did not log in")
}

/// A phone at `directory` that logged in before, opened again.
pub(crate) fn reopen(directory: &Path) -> Result<(Arc<Device>, World)> {
    let cache = fastctx_client::cache::Cache::open(directory.join("cache"))?;
    let device = Device::open_managed(directory.join("device"), cache.clone())?;
    Ok((device.clone(), World::from_device(device).keeping(cache)))
}

pub(crate) async fn build(config: Config) -> Result<Lab> {
    for dir in ["user", "claude", "phones"] {
        std::fs::create_dir_all(config.base.join(dir))?;
    }
    std::fs::create_dir_all(&config.world)?;
    let listen = free()?;
    let hub_dir = config.base.join("hub");
    fastctx_hub::init(
        &hub_dir,
        fastctx_hub::New {
            name: "soak".parse()?,
            public: vec![listen],
            listen,
            admin: account("alice"),
            password: PASS.into(),
        },
    )?;
    let (out, err) = output(&config.base.join("hub.out"))?;
    let child = Command::new(&config.kernel)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", config.base.join("user"))
        .env("LANG", "C.UTF-8")
        .env("FASTCTX_UPDATE_RELEASES", "http://127.0.0.1:9/releases")
        .args(["hub", "serve", "--dir"])
        .arg(&hub_dir)
        .stdin(Stdio::null())
        .stdout(out)
        .stderr(err)
        .spawn()
        .context("starting the hub")?;
    let hub = Hub { dir: hub_dir, child };
    for _ in 0..200 {
        if tokio::net::TcpStream::connect(listen).await.is_ok() {
            break;
        }
        pause().await;
    }
    let node_line = Line::open(listen).await?;
    let device_line = Line::open(listen).await?;
    let (owner_device, owner) = phone(&config.base.join("owner-phone"), "alice", listen).await?;
    owner
        .account(
            None,
            account("alice"),
            AccountChange::OpenAccount { name: account("bob"), password: PASS.into(), visitor: true },
        )
        .await
        .context("opening bob's account")?;

    let patrol_root = config.base.join(PATROL);
    join(&config, &patrol_root, PATROL, "alice", listen, listen).await?;
    let (out, err) = output(&patrol_root.join("stdout.log"))?;
    let patrol = command(&config, &patrol_root, &patrol_root.join("bin/fastctx"), &config.base.join("user"))
        .arg("node")
        .stdout(out)
        .stderr(err)
        .spawn()
        .context("starting the patrol machine")?;
    anyhow::ensure!(answers(&patrol_root, Duration::from_secs(60), true).await, "the patrol machine did not come up");

    join(&config, &config.world, SOAK, "alice", node_line.address, node_line.address).await?;
    let soak = Soak {
        config: config.clone(),
        systemd_run: found("systemd-run")?,
        child: Arc::default(),
        pid: Arc::default(),
        ended: Arc::default(),
    };
    soak.start()?;
    anyhow::ensure!(
        answers(&config.world, Duration::from_secs(120), true).await,
        "the node under test did not come up"
    );
    let local = World::new(WorldDir::under(&config.world), Peer::Command { resident: None, session: None });

    let agent: AgentName = "codex".parse()?;
    local
        .add_agent_login(machine(SOAK).into(), agent.clone(), config.codex_home.clone())
        .await
        .context("adding the mock Codex login")?;
    local
        .change_agent_login(machine(SOAK).into(), agent.clone(), config.codex_home.clone(), true, true, true)
        .await
        .context("enabling the mock Codex login")?;
    owner
        .account(
            None,
            account("alice"),
            AccountChange::Assist {
                machine: machine(SOAK),
                to: account("bob"),
                agent: agent.clone(),
                home: Some(config.codex_home.clone()),
                assisting: true,
                until: None,
                weekly: None,
                daily: None,
                five_hour: None,
                turns: None,
            },
        )
        .await
        .context("letting bob speak through the login")?;

    // Half the phones are alice's, half bob's, each logged in once and
    // opened again whenever it comes back.
    let mut phones = Vec::new();
    for index in 0..config.devices {
        let owner = if index % 2 == 0 { "alice" } else { "bob" };
        let directory = config.base.join("phones").join(format!("phone-{index}"));
        let (device, _) = phone(&directory, owner, device_line.address).await?;
        device.disconnect();
        phones.push((directory, account(owner)));
    }
    Ok(Lab {
        config,
        hub,
        node_line,
        device_line,
        soak,
        patrol,
        patrol_root,
        local,
        owner,
        _owner_device: owner_device,
        phones,
        agent,
    })
}
