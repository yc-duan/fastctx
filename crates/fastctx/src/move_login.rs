//! `fastctx move-login`: moves a login's directory — Claude Code's or
//! Codex's — to another disk, and leaves a link where it was, so every path
//! written to it, by the agent or by fastctx, holds as it is (the user,
//! 2026-11-21: "他要搬迁过去，但是允许你在原位置有一些残留，这样更简单不容易出错").
//! The machine's node is stopped throughout, its lock held, and the apply
//! lock with it: nothing of fastctx touches the login meanwhile. The
//! directory is copied beside its target with all a filesystem keeps of it,
//! checked entry by entry, put in the target's place, then the original is
//! set aside beside where it was and the link takes its name: that rename is
//! the one step after which the move goes forward. The Claude receipts kept
//! by the directory's real place follow it, and the login keeps its id
//! (`fastctx_node::login_moves`). The original stays set aside until it is
//! deleted at the user's word, checked unchanged first. Linux only, in this
//! version.

use std::path::PathBuf;

use clap::Args;

#[cfg(target_os = "linux")]
mod occupants;
#[cfg(target_os = "linux")]
mod tree;

#[derive(Args)]
pub struct MoveLoginArgs {
    /// The login's directory, as this machine's node knows it: `~/.claude`,
    /// a `CLAUDE_CONFIG_DIR`, `~/.codex` or a `CODEX_HOME`.
    #[arg(value_parser = crate::paths::parser())]
    login: Option<PathBuf>,
    /// Where it goes: an absolute path not there yet, on the disk it goes to.
    #[arg(value_parser = crate::paths::parser())]
    target: Option<PathBuf>,
    /// Stop this machine's node, and with it every conversation on it, while
    /// the directory moves.
    #[arg(long)]
    yes: bool,
    /// The target's disk need not be one mounted at boot.
    #[arg(long)]
    unmounted_at_boot_ok: bool,
    /// Stop the agents still using the login — Claude Code, Codex — and what
    /// they started, rather than wait for them to end: what they were doing
    /// is cut short. Anything else using it stops the move all the same.
    #[arg(long)]
    stop_agents: bool,
    /// Finish a move that was cut short.
    #[arg(long, conflicts_with_all = ["login", "target", "drop_backup", "status"])]
    resume: bool,
    /// Delete the original a finished move set aside, by the move's id.
    #[arg(long, value_name = "MOVE", conflicts_with_all = ["login", "target", "status"])]
    drop_backup: Option<String>,
    /// With --drop-backup: though the original changed after it was copied.
    #[arg(long, requires = "drop_backup")]
    anyway: bool,
    /// The moves recorded on this machine.
    #[arg(long, conflicts_with_all = ["login", "target"])]
    status: bool,
    /// With --status: as JSON.
    #[arg(long, requires = "status")]
    json: bool,
    /// The machine this is meant for, by its name in the World: refused
    /// where this machine's World is another's (the machine page asks so
    /// over SSH).
    #[arg(long, hide = true)]
    machine: Option<String>,
    /// With --machine: the network it is meant on, by its hub's id — one of
    /// another network of the same name is not it.
    #[arg(long, hide = true, requires = "machine")]
    hub: Option<String>,
}

pub fn run(args: MoveLoginArgs) -> anyhow::Result<()> {
    #[cfg(target_os = "linux")]
    return linux::run(args);
    #[cfg(not(target_os = "linux"))]
    {
        let _ = args;
        anyhow::bail!("a login's directory is moved on Linux in this version of fastctx")
    }
}

#[cfg(target_os = "linux")]
pub(crate) mod linux {
    use std::fs;
    use std::os::unix::fs::MetadataExt;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    use anyhow::{Context, anyhow, bail};
    use fastctx_install::service::{Kind as ServiceKind, Service};
    use fastctx_node::login_moves::{self as moves, Copied, Identity, Phase, Record, ServiceBefore};
    use fastctx_platform::FileLock;
    use fastctx_types::layout::WorldDir;

    use super::MoveLoginArgs;
    use super::{occupants, tree};

    /// What a run says first of the move it is, for whoever asked over SSH.
    pub(crate) const MOVE_SAID: &str = "fastctx-move: ";

    /// This machine's fastctx: its root and World, the user and group that
    /// own them, and the service that keeps its node, where one does.
    pub(crate) struct Here {
        root: PathBuf,
        pub(crate) world: WorldDir,
        owner: u32,
        group: u32,
        home: PathBuf,
        pub(crate) service: Option<Service>,
    }

    impl Here {
        pub(crate) fn find() -> anyhow::Result<Self> {
            let root = fastctx_platform::fastctx_root()?;
            let world = WorldDir::under(&root);
            let metadata = fs::metadata(&root).with_context(|| format!("reading {}", root.display()))?;
            let (owner, group) = (metadata.uid(), metadata.gid());
            // SAFETY: geteuid has no preconditions.
            let caller = unsafe { libc::geteuid() };
            if caller != 0 && caller != owner {
                bail!("{} is another user's; run this as that user or as root", root.display());
            }
            let home = fastctx_platform::unix_account::lookup(owner)?
                .map(|account| PathBuf::from(account.home))
                .ok_or_else(|| anyhow!("user {owner} has no account record"))?;
            let service = world.node_service().try_exists()?.then(|| Service {
                uid: owner,
                ..Service::new(ServiceKind::Node, fastctx_install::executable::installed_path(&root), root.clone())
            });
            if service.is_some() && caller != 0 {
                bail!("this machine's node is kept by a system service, which only root stops: run this as root");
            }
            Ok(Self { root, world, owner, group, home, service })
        }

        fn records(&self) -> PathBuf {
            self.world.path().join(moves::DIRECTORY)
        }

        /// Gives what this command made in the World to the World's owner:
        /// run by root for another user's World, its node reads it as that
        /// user.
        pub(crate) fn own(&self, path: &Path) -> anyhow::Result<()> {
            // SAFETY: geteuid has no preconditions.
            if unsafe { libc::geteuid() } != 0 || self.owner == 0 {
                return Ok(());
            }
            std::os::unix::fs::lchown(path, Some(self.owner), Some(self.group))
                .with_context(|| format!("giving {} to the World's owner", path.display()))
        }
    }

    /// Refuses to run from inside this machine's fastctx — a conversation's
    /// shell, a command its node started — which stopping the node would stop
    /// with it (the user, 2026-11-21: "这里是不是控制端和被控端是一个，被控端
    /// 停止了，控制端是不是也停了"): it is run in a login of this machine's
    /// own, over ssh.
    pub(crate) fn outside_fastctx() -> anyhow::Result<()> {
        let refusal = "this runs inside this machine's fastctx (a conversation, or a command its node started), which stops with the node: log in to this machine directly — over ssh — and run it there";
        if std::env::var_os("FASTCTX_RESIDENT").is_some() {
            bail!(refusal);
        }
        let mut pid = std::os::unix::process::parent_id();
        for _ in 0..128 {
            if pid <= 1 {
                break;
            }
            // A node: fastctx's program, run as `fastctx node`.
            let command = occupants::command(pid);
            let fastctx = |path: &Path| path.file_name().is_some_and(|name| name.to_string_lossy().starts_with("fastctx"));
            let named = occupants::program(pid).is_some_and(|program| fastctx(&program))
                || command.first().is_some_and(|first| fastctx(Path::new(first)));
            if named && command.get(1).is_some_and(|word| word == "node") {
                bail!(refusal);
            }
            let Some((parent, _)) = occupants::stat(pid) else { break };
            pid = parent;
        }
        Ok(())
    }

    pub fn run(args: MoveLoginArgs) -> anyhow::Result<()> {
        let here = Here::find()?;
        if let Some(meant) = &args.machine {
            let file = fs::read(here.world.machine_file()).context("reading this machine's name in the World")?;
            let name = serde_json::from_slice::<serde_json::Value>(&file)
                .ok()
                .and_then(|file| file.get("name").and_then(|name| name.as_str()).map(str::to_string))
                .ok_or_else(|| anyhow!("this machine's name in the World cannot be read"))?;
            if name != *meant {
                bail!("this machine is {name} in the World, not {meant}: nothing is done here");
            }
            if let Some(hub) = &args.hub {
                let plain = !hub.is_empty() && hub.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'));
                // On that network still: one the hub ended this machine's
                // place on — its name given to another since — is not it.
                let place = here.world.net().join(hub);
                let on = plain && fs::metadata(&place).is_ok_and(|found| found.is_dir()) && !place.join("ended.json").exists();
                if !on {
                    bail!("this machine is not {meant} of the network meant (hub {hub}): nothing is done here");
                }
            }
        }
        if args.status {
            return status(&here, args.json);
        }
        outside_fastctx()?;
        let _alone = alone(&here)?;
        if let Some(op) = &args.drop_backup {
            return drop_backup(&here, op, args.anyway);
        }
        if args.resume {
            return resume(&here);
        }
        let (Some(login), Some(target)) = (args.login, args.target) else {
            bail!("name the login's directory and where it goes, or --resume, --drop-backup or --status");
        };
        if !args.yes {
            bail!("moving a login stops this machine's node, and every conversation on it, until it is done: pass --yes");
        }
        if let Some((path, _)) = moves::records(here.world.path())?.into_iter().find(|(_, record)| moves::pending(record)) {
            bail!("a move is unfinished ({}); finish it with `fastctx move-login --resume`", path.display());
        }
        if there(&crate::compact::linux::record(&here))? {
            bail!("a compaction was cut short; `fastctx data compact --resume` puts the node's service back first");
        }
        let moved = start(&here, &login, &target, args.unmounted_at_boot_ok, args.stop_agents);
        if moved.is_err() && unfinished(&here) {
            eprintln!("fastctx: the move is unfinished; `fastctx move-login --resume` finishes it");
        }
        moved
    }

    /// The moves recorded on this World, read and nothing made.
    fn status(here: &Here, json: bool) -> anyhow::Result<()> {
        if json {
            let records: Vec<Record> = moves::records(here.world.path())?.into_iter().map(|(_, record)| record).collect();
            println!("{}", serde_json::to_string_pretty(&records)?);
            return Ok(());
        }
        for (path, record) in moves::records(here.world.path())? {
            println!(
                "{}  {:?}{}  {} -> {}{}{}  ({})",
                record.op,
                record.phase,
                if record.service_restored { "" } else { " (service not put back)" },
                record.login.display(),
                record.target.display(),
                if record.phase == Phase::Done && !record.backup_deleted {
                    format!("  backup {}", record.backup.display())
                } else {
                    String::new()
                },
                record.why.as_deref().map(|why| format!("  {why}")).unwrap_or_default(),
                path.display()
            );
        }
        Ok(())
    }

    /// Whether a move is left to finish, or its service to put back.
    pub(crate) fn unfinished(here: &Here) -> bool {
        moves::records(here.world.path()).map_or(true, |records| records.iter().any(|(_, record)| moves::pending(record)))
    }

    fn identity(path: &Path) -> std::io::Result<Identity> {
        let metadata = fs::symlink_metadata(path)?;
        Ok(Identity { device: metadata.dev(), inode: metadata.ino() })
    }

    /// The agent whose login `dir` is, by what it holds.
    fn agent_of(dir: &Path) -> anyhow::Result<&'static str> {
        let has = |name: &str| dir.join(name).symlink_metadata().is_ok();
        let codex = has("config.toml") || has("auth.json") || has("sessions");
        let claude = has("settings.json") || has(".credentials.json") || has("projects") || has(".claude.json");
        match (claude, codex) {
            (true, false) => Ok("claude"),
            (false, true) => Ok("codex"),
            (true, true) => bail!("{} holds both Claude Code's and Codex's things; it is not moved", dir.display()),
            (false, false) => bail!("{} holds nothing of Claude Code or Codex", dir.display()),
        }
    }

    /// The directories the World's logins were given ids for, with their agents.
    fn registered(world: &Path) -> anyhow::Result<Vec<(String, PathBuf)>> {
        #[derive(serde::Deserialize)]
        struct Registry {
            logins: Vec<Login>,
        }
        #[derive(serde::Deserialize)]
        struct Login {
            agent: String,
            dir: PathBuf,
        }
        match fs::read(world.join("login-ids.json")) {
            Ok(bytes) => Ok(serde_json::from_slice::<Registry>(&bytes)
                .context("reading login-ids.json")?
                .logins
                .into_iter()
                .map(|login| (login.agent, login.dir))
                .collect()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(error) => Err(error).context("reading login-ids.json"),
        }
    }

    /// Whether `point`, where a filesystem is mounted, is mounted at boot:
    /// the root, or named in `/etc/fstab`.
    fn mounted_at_boot(point: &Path) -> bool {
        if point == Path::new("/") {
            return true;
        }
        fs::read_to_string("/etc/fstab").is_ok_and(|table| {
            table
                .lines()
                .map(str::trim)
                .filter(|line| !line.starts_with('#'))
                .filter_map(|line| line.split_whitespace().nth(1))
                .any(|named| Path::new(named) == point)
        })
    }

    fn mount_point(path: &Path) -> anyhow::Result<PathBuf> {
        let mounts = fastctx_platform::mountinfo::read()?;
        Ok(mounts
            .into_iter()
            .map(|mount| mount.point)
            .filter(|point| path.starts_with(point))
            .max_by_key(|point| point.components().count())
            .unwrap_or_else(|| PathBuf::from("/")))
    }

    /// The places a filesystem is mounted at `path` or inside it.
    fn mounted_in(path: &Path) -> anyhow::Result<Vec<PathBuf>> {
        Ok(fastctx_platform::mountinfo::read()?.into_iter().map(|mount| mount.point).filter(|point| point.starts_with(path)).collect())
    }

    fn keep(here: &Here, record: &Record) -> anyhow::Result<()> {
        let path = here.records().join(format!("{}.json", record.op));
        moves::keep(&path, record).with_context(|| format!("keeping the record of the move {}", record.op))?;
        here.own(&path)?;
        // Rehearsals cut a move short where they want it cut, as a power
        // failure would, just after a phase is kept.
        if let Some(stop) = std::env::var_os("FASTCTX_LAB_MOVE_LOGIN_STOP")
            && stop.to_string_lossy() == format!("{:?}", record.phase)
        {
            eprintln!("rehearsal: stopped after {:?}", record.phase);
            std::process::exit(9);
        }
        Ok(())
    }

    fn manifest_path(here: &Here, op: &str) -> PathBuf {
        here.records().join(format!("{op}.manifest"))
    }

    /// What the move holds while it runs: the node's lock and the apply
    /// lock. Let go before the service is started again.
    pub(crate) struct Held {
        _node: FileLock,
        _apply: FileLock,
    }

    /// Stops the node's service, where one keeps it, and takes the node's
    /// lock and the apply lock, waiting a while for each.
    pub(crate) fn hold(here: &Here) -> anyhow::Result<Held> {
        if let Some(service) = &here.service {
            service.hold().context("stopping the node's service")?;
        }
        let take = |path: PathBuf, what: &str| -> anyhow::Result<FileLock> {
            let deadline = Instant::now() + Duration::from_secs(120);
            loop {
                if let Some(lock) = FileLock::try_acquire(&path)? {
                    return Ok(lock);
                }
                if Instant::now() >= deadline {
                    bail!("{what} was not let go within two minutes: stop what holds it and try again");
                }
                std::thread::sleep(Duration::from_millis(250));
            }
        };
        let node = take(here.world.node_lock(), "the World (a node still serves it)")?;
        let apply = take(here.root.join("apply.lock"), "the apply lock (an apply runs)")?;
        Ok(Held { _node: node, _apply: apply })
    }

    /// Puts the node's service back as it stood before the move, the locks
    /// let go first.
    fn restore(here: &Here, record: &mut Record, held: Option<Held>) -> anyhow::Result<()> {
        drop(held);
        put_service_back(here, record.service.as_ref())?;
        record.service_restored = true;
        keep(here, record)
    }

    /// Puts the node's service back as `before` says it stood: enabled, it is
    /// enabled and started again — its check would start it anyway — and
    /// held, it is left held; one held yet running then is said, not started.
    pub(crate) fn put_service_back(here: &Here, before: Option<&ServiceBefore>) -> anyhow::Result<()> {
        let (Some(service), Some(before)) = (&here.service, before) else { return Ok(()) };
        if before.enabled {
            service.release().context("starting the node's service again")?;
        } else if before.active {
            println!(
                "the node's service ran though it was held (disabled) before; it is left stopped: `systemctl start {}` starts it as it was",
                before.name
            );
        } else {
            println!("the node's service was held before; it is left held");
        }
        Ok(())
    }

    /// The lock of the maintenance done one at a time on this World — a
    /// login's move, the library's compaction — taken at once, or refused.
    pub(crate) fn alone(here: &Here) -> anyhow::Result<FileLock> {
        let records = here.records();
        fastctx_platform::ensure_private_dir(&records)?;
        here.own(&records)?;
        let lock = records.join(".lock");
        let alone = FileLock::try_acquire(&lock)?
            .ok_or_else(|| anyhow!("another `fastctx move-login` or `fastctx data compact` runs on this World"))?;
        here.own(&lock)?;
        Ok(alone)
    }

    /// Whether anything is at `path`; an error where that cannot be told.
    pub(crate) fn there(path: &Path) -> anyhow::Result<bool> {
        match fs::symlink_metadata(path) {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error).with_context(|| format!("looking at {}", path.display())),
        }
    }

    /// What an apply left unfinished, or a receipt in the way of the Claude
    /// receipts following the directory: a reason not to move it now.
    fn applies_in_the_way(here: &Here, agent: &str, target: &Path) -> anyhow::Result<()> {
        let left = fastctx_install::apply::moved::unfinished(&here.root)?;
        if !left.is_empty() {
            bail!("an apply was left unfinished ({}); run `fastctx apply` to finish it first", left.join(", "));
        }
        if agent == "claude" {
            fastctx_install::apply::moved::conflicts(&here.root, target)?;
        }
        Ok(())
    }

    fn start(here: &Here, login: &Path, target: &Path, unmounted_ok: bool, stop_agents: bool) -> anyhow::Result<()> {
        let login = std::path::absolute(login)?;
        let source = fs::canonicalize(&login).with_context(|| format!("resolving {}", login.display()))?;
        if !fs::metadata(&source)?.is_dir() {
            bail!("{} is not a directory", source.display());
        }
        let agent = agent_of(&source)?;
        if !target.is_absolute() {
            bail!("{} is not an absolute path", target.display());
        }
        match fs::symlink_metadata(target) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Ok(_) => bail!("{} is there already", target.display()),
            Err(error) => return Err(error).with_context(|| format!("looking at {}", target.display())),
        }
        let name = target.file_name().ok_or_else(|| anyhow!("{} names no directory", target.display()))?;
        let parent = fs::canonicalize(target.parent().expect("an absolute path's parent"))
            .with_context(|| format!("resolving the directory {} goes in", target.display()))?;
        let target = parent.join(name);
        if target.starts_with(&source) || source.starts_with(&target) {
            bail!("{} and {} hold one another", source.display(), target.display());
        }
        for (what, path) in [("fastctx's directory", &here.root), ("the World", &here.world.path().to_path_buf())] {
            let real = fs::canonicalize(path).unwrap_or_else(|_| path.clone());
            if real.starts_with(&source) {
                bail!("{what}, {}, is in {}: it cannot be moved with the node stopped", real.display(), source.display());
            }
        }
        if let Some(point) = mounted_in(&source)?.first() {
            bail!(
                "a filesystem is mounted at {}, {}: what is mounted is not the login's to move, nor to delete with its original",
                point.display(),
                if *point == source { "the login's own directory" } else { "inside the login's directory" }
            );
        }
        for (other, dir) in registered(here.world.path())? {
            if dir == target {
                bail!("{} is already the directory of another login", target.display());
            }
            if dir != source && (dir.starts_with(&source) || source.starts_with(&dir)) {
                bail!("{} holds, or is held by, another login's directory, {}; they are not moved apart", source.display(), dir.display());
            }
            if dir == source && other != agent {
                bail!("{} is {other}'s login as well as {agent}'s; it is not moved", source.display());
            }
        }
        applies_in_the_way(here, agent, &target)?;
        let point = mount_point(&parent)?;
        if !mounted_at_boot(&point) && !unmounted_ok {
            bail!(
                "{} is on the filesystem mounted at {}, which /etc/fstab does not mount at boot: the login would be missing after a restart (--unmounted-at-boot-ok to move it all the same)",
                target.display(),
                point.display()
            );
        }
        let entries = tree::walk(&source).with_context(|| format!("reading {}", source.display()))?;
        // SAFETY: geteuid has no preconditions.
        let superuser = unsafe { libc::geteuid() } == 0;
        let findings = tree::findings(&source, &entries, superuser, here.owner);
        if let Some(foreign) = findings.foreign.first() {
            bail!("{} is another user's; only root moves it with its owner kept", source.join(foreign).display());
        }
        if let Some(mounted) = findings.mounted.first() {
            bail!("{} is on another filesystem than the login's directory; it is not moved", source.join(mounted).display());
        }
        if !findings.unsure.is_empty() {
            let unsure: Vec<String> = findings.unsure.iter().map(|path| source.join(path).display().to_string()).collect();
            bail!(
                "these links lead out of the directory to nowhere, through names whose place cannot be told: {}; mend or remove them first",
                unsure.join(", ")
            );
        }
        let available = fastctx_platform::space::available(&parent)?;
        let needed = findings.bytes + findings.bytes / 10 + (1 << 30);
        if available < needed {
            bail!("{} has {} MiB free; {} MiB are needed", parent.display(), available >> 20, needed >> 20);
        }
        for other in &findings.others {
            println!("not copied, as no copy of it means anything: {}", source.join(other).display());
        }
        for (at, led) in &findings.outward {
            println!("{} leads out of the directory; the copy leads to {}", source.join(at).display(), led.display());
        }
        let op = format!("{}-{:08x}", jiff::Timestamp::now().strftime("%Y%m%dT%H%M%S"), {
            let mut bytes = [0_u8; 4];
            getrandom::fill(&mut bytes)?;
            u32::from_le_bytes(bytes)
        });
        let file_name = source.file_name().ok_or_else(|| anyhow!("{} names no directory", source.display()))?.to_string_lossy().into_owned();
        let backup = source.with_file_name(format!("{file_name}.fastctx-moved-{op}"));
        let staging = parent.join(format!(".fastctx-move-{op}"));
        let service = match &here.service {
            Some(service) => {
                let status = service.status()?;
                Some(ServiceBefore {
                    name: status.name,
                    enabled: status.enabled,
                    active: status.state.starts_with("active"),
                })
            }
            None => None,
        };
        let mut record = Record {
            op: op.clone(),
            agent: agent.to_string(),
            login: login.clone(),
            source: source.clone(),
            source_identity: identity(&source)?,
            target: target.clone(),
            target_identity: None,
            staging: staging.clone(),
            backup,
            service,
            service_restored: false,
            phase: Phase::Stopping,
            copied: None,
            why: None,
            at: jiff::Timestamp::now(),
            backup_deleted: false,
        };
        keep(here, &record)?;
        // Read by whoever asked over SSH: the move this run is.
        println!("{MOVE_SAID}{op}");
        println!("moving {} ({agent}) to {}: the node is stopped until it is done (move {op})", login.display(), target.display());
        let held = match hold(here) {
            Ok(held) => held,
            Err(error) => return abort(here, &mut record, None, &format!("{error:#}")),
        };
        // Looked at again under the apply lock: an apply that was under way
        // as this began may have stopped half done.
        if let Err(error) = applies_in_the_way(here, agent, &target) {
            return abort(here, &mut record, Some(held), &format!("{error:#}"));
        }
        record.phase = Phase::Checking;
        keep(here, &record)?;
        if let Err(error) = check_unused(here, &record, stop_agents) {
            return abort(here, &mut record, Some(held), &format!("{error:#}"));
        }
        record.phase = Phase::Copying;
        keep(here, &record)?;
        let copied = (|| -> anyhow::Result<tree::Copy> {
            tree::stage(&staging, &op)?;
            let copied = tree::copy(&source, &staging, &entries, &findings.outward)?;
            let manifest = manifest_path(here, &op);
            fastctx_platform::publish(&manifest, copied.manifest.as_bytes(), fastctx_platform::Permissions::Private)?;
            here.own(&manifest)?;
            Ok(copied)
        })();
        let copied = match copied {
            Ok(copied) => copied,
            Err(error) => return abort(here, &mut record, Some(held), &format!("copying: {error:#}")),
        };
        record.copied = Some(Copied { entries: copied.entries, bytes: copied.bytes, digest: tree::digest(&copied.manifest) });
        record.phase = Phase::Verifying;
        keep(here, &record)?;
        match tree::differences(&source, &staging, &entries, &copied, &findings.outward) {
            Ok(differs) if differs.is_empty() => {}
            Ok(differs) => return abort(here, &mut record, Some(held), &format!("the copy is not the original: {}", differs.join("; "))),
            Err(error) => return abort(here, &mut record, Some(held), &format!("checking the copy: {error:#}")),
        }
        if let Err(error) = tree::rename_new(&staging, &target) {
            return abort(here, &mut record, Some(held), &format!("putting the copy at {}: {error}", target.display()));
        }
        record.target_identity = Some(identity(&target)?);
        record.phase = Phase::Published;
        keep(here, &record)?;
        // Once more, just before the original is set aside.
        if let Err(error) = check_unused(here, &record, stop_agents) {
            return abort(here, &mut record, Some(held), &format!("{error:#}"));
        }
        let now = tree::walk(&source)?;
        if now.len() != entries.len()
            || now.iter().zip(&entries).any(|(now, then)| now.path != then.path || now.mtime != then.mtime || now.ctime != then.ctime || now.size != then.size)
        {
            return abort(here, &mut record, Some(held), "the original changed after it was copied");
        }
        record.phase = Phase::Swapping;
        keep(here, &record)?;
        // The one step after which the move goes forward.
        match tree::rename_new(&source, &record.backup) {
            Ok(()) => {}
            Err(tree::Renamed::Not(error)) => {
                return abort(here, &mut record, Some(held), &format!("setting {} aside: {error}", source.display()));
            }
            Err(error) => {
                drop(held);
                bail!("setting {} aside: {error}; nothing more is done until `fastctx move-login --resume`", source.display());
            }
        }
        link(here, &record)?;
        forward(here, &mut record, held)
    }

    /// Puts the link to the target where the source was: made beside it,
    /// then renamed into its place, which must be empty.
    fn link(here: &Here, record: &Record) -> anyhow::Result<()> {
        let beside = record.source.with_file_name(format!(".fastctx-link-{}", record.op));
        match fs::symlink_metadata(&beside) {
            Ok(metadata) if metadata.file_type().is_symlink() => fs::remove_file(&beside)?,
            Ok(_) => bail!("{} is there and is no link of this move's", beside.display()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        std::os::unix::fs::symlink(&record.target, &beside).with_context(|| format!("making the link {}", beside.display()))?;
        here.own(&beside)?;
        tree::rename_new(&beside, &record.source)
            .map_err(|error| anyhow!("putting the link at {}: {error}; `fastctx move-login --resume` goes on", record.source.display()))
    }

    /// From the link on: the receipts follow the directory, the marker goes,
    /// the service is put back.
    fn forward(here: &Here, record: &mut Record, held: Held) -> anyhow::Result<()> {
        record.phase = Phase::Linked;
        keep(here, record)?;
        if record.agent == "claude" {
            for done in fastctx_install::apply::moved::moved(&here.root, &record.source, &record.target)? {
                println!("{done}");
            }
        }
        record.phase = Phase::Done;
        keep(here, record)?;
        unmark(record)?;
        restore(here, record, Some(held))?;
        println!("moved: {} -> {} (move {})", record.login.display(), record.target.display(), record.op);
        println!("the original is kept at {} until `fastctx move-login --drop-backup {}`", record.backup.display(), record.op);
        println!("check before deleting it: the login keeps its id; old conversations go on (also /resume, --continue, --resume <id> in Claude Code); plugins and MCP servers; usage");
        Ok(())
    }

    /// Takes the move's marker out of the target, where it is still.
    fn unmark(record: &Record) -> anyhow::Result<()> {
        match fs::remove_file(record.target.join(tree::MARKER)) {
            Ok(()) => fastctx_platform::durable_file::sync_directory(&record.target).context("taking the move's marker out"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error).context("taking the move's marker out"),
        }
    }

    /// Who uses the source or the target: an error saying who, after a while
    /// for them to end; the agents among them stopped at once where
    /// `stop_agents`.
    fn check_unused(here: &Here, record: &Record, stop_agents: bool) -> anyhow::Result<()> {
        let default = match record.agent.as_str() {
            "claude" => here.home.join(".claude"),
            _ => here.home.join(".codex"),
        };
        let default = record.login == default || record.source == fs::canonicalize(&default).unwrap_or(default);
        let deadline = Instant::now() + Duration::from_secs(60);
        let mut rounds = 0;
        loop {
            let mut found = occupants::of(&record.source, &record.agent, default, here.owner);
            if record.target_identity.is_some() {
                found.extend(occupants::of(&record.target, &record.agent, false, here.owner));
            }
            let mut seen = std::collections::BTreeSet::new();
            found.retain(|occupant| seen.insert(occupant.pid));
            if found.is_empty() {
                return Ok(());
            }
            // Only what is shown to use it: one that cannot be looked at is
            // said, never stopped on a guess.
            if stop_agents && rounds < 3 && found.iter().any(|occupant| occupant.agent && occupant.proven) {
                rounds += 1;
                for occupant in found.iter().filter(|occupant| occupant.agent && occupant.proven) {
                    println!("stopping {}", occupant.said());
                    occupants::stop(occupant).map_err(|error| anyhow!("{} could not be stopped: {error}", occupant.said()))?;
                }
                continue;
            }
            if Instant::now() >= deadline {
                let said: Vec<String> = found.iter().map(|occupant| occupant.said()).collect();
                bail!(
                    "the login is in use; stop these first (and whatever starts them again){}:\n  {}",
                    if stop_agents { "" } else { ", or pass --stop-agents to stop the agents among them" },
                    said.join("\n  ")
                );
            }
            std::thread::sleep(Duration::from_secs(2));
        }
    }

    /// A move given up before its original was touched: taken back, its
    /// service put back; `clean` where nothing it made is left behind.
    #[derive(Debug)]
    struct TakenBack {
        why: String,
        clean: bool,
    }

    impl std::fmt::Display for TakenBack {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "the login was not moved: {}", self.why)
        }
    }

    impl std::error::Error for TakenBack {}

    /// Gives a move up before its original was touched: what it made goes,
    /// the service is put back.
    fn abort(here: &Here, record: &mut Record, held: Option<Held>, why: &str) -> anyhow::Result<()> {
        let undone = undo(record);
        record.phase = Phase::Aborted;
        record.why = Some(match &undone {
            Ok(()) => why.to_string(),
            Err(error) => format!("{why}; and what it made could not all be taken away: {error:#}"),
        });
        keep(here, record)?;
        restore(here, record, held)?;
        Err(TakenBack { why: record.why.clone().unwrap_or_default(), clean: undone.is_ok() }.into())
    }

    /// Takes away the staging directory and the published copy this move
    /// made, the original untouched: the copy by what it is or by the move's
    /// marker in it, never for being empty alone.
    fn undo(record: &Record) -> anyhow::Result<()> {
        if fs::symlink_metadata(&record.staging).is_ok() {
            if !tree::staged_by(&record.staging, &record.op) {
                bail!("{} is not this move's; it is left", record.staging.display());
            }
            fastctx_platform::owned_directory::delete(&record.staging)?;
        }
        if let Ok(metadata) = fs::symlink_metadata(&record.target)
            && metadata.is_dir()
            && (Some(Identity { device: metadata.dev(), inode: metadata.ino() }) == record.target_identity
                || tree::marked_by(&record.target, &record.op))
        {
            fastctx_platform::owned_directory::delete(&record.target)?;
        }
        Ok(())
    }

    /// How the three places of a move stand on disk now; `unreadable` where
    /// any of them could not be looked at, there or not.
    struct Standing {
        source_is_original: bool,
        source_is_link: bool,
        source_gone: bool,
        backup: Option<Identity>,
        target_is_ours: bool,
        unreadable: bool,
    }

    impl Standing {
        fn of(record: &Record) -> Self {
            let source = fs::symlink_metadata(&record.source);
            let target = identity(&record.target).ok();
            let unknown = |path: &Path| {
                matches!(fs::symlink_metadata(path), Err(error) if error.kind() != std::io::ErrorKind::NotFound)
            };
            Self {
                unreadable: unknown(&record.source) || unknown(&record.backup) || unknown(&record.target),
                source_is_original: source.as_ref().is_ok_and(|metadata| {
                    metadata.is_dir() && Identity { device: metadata.dev(), inode: metadata.ino() } == record.source_identity
                }),
                source_is_link: source.as_ref().is_ok_and(|metadata| metadata.file_type().is_symlink())
                    && fs::read_link(&record.source).is_ok_and(|led| led == record.target),
                source_gone: matches!(&source, Err(error) if error.kind() == std::io::ErrorKind::NotFound),
                backup: identity(&record.backup).ok(),
                target_is_ours: target.is_some() && target == record.target_identity,
            }
        }

        fn backup_is_original(&self, record: &Record) -> bool {
            self.backup == Some(record.source_identity)
        }
    }

    fn cannot_tell(record: &Record) -> anyhow::Error {
        anyhow!(
            "the move cannot tell how to go on: {} is {}, {} is {}, {} is {}; nothing is changed — look, put things right, and run --resume again",
            record.source.display(),
            describe(&record.source),
            record.backup.display(),
            describe(&record.backup),
            record.target.display(),
            describe(&record.target)
        )
    }

    /// Finishes the move cut short, from where it stands on disk.
    fn resume(here: &Here) -> anyhow::Result<()> {
        let found = moves::records(here.world.path())?.into_iter().find(|(_, record)| moves::pending(record));
        let Some((_, mut record)) = found else {
            println!("no move is unfinished");
            return Ok(());
        };
        println!("{MOVE_SAID}{}", record.op);
        println!("finishing the move {} of {} to {} ({:?})", record.op, record.login.display(), record.target.display(), record.phase);
        // Taken back with nothing of it left: finished as well as it can be.
        let taken_back = |result: anyhow::Result<()>| -> anyhow::Result<()> {
            match result {
                Err(error) => match error.downcast_ref::<TakenBack>() {
                    Some(taken) if taken.clean => {
                        println!("{taken}");
                        Ok(())
                    }
                    _ => Err(error),
                },
                Ok(()) => Ok(()),
            }
        };
        match record.phase {
            // Settled: the service is left to put back.
            Phase::Aborted => return restore(here, &mut record, None),
            // Done but for the service: put back only where the login leads
            // to the copy still, as the move left it.
            Phase::Done => {
                let standing = Standing::of(&record);
                let led = fs::canonicalize(&record.login).ok().is_some_and(|real| Some(real) == fs::canonicalize(&record.target).ok());
                if standing.unreadable || !(standing.source_is_link && standing.target_is_ours) || !led {
                    return Err(cannot_tell(&record));
                }
                unmark(&record)?;
                return restore(here, &mut record, None);
            }
            _ => {}
        }
        let held = hold(here)?;
        let standing = Standing::of(&record);
        match record.phase {
            // Before the original was set aside: it stands where it was, or
            // nothing of this move's is taken away.
            Phase::Stopping | Phase::Checking | Phase::Copying | Phase::Verifying | Phase::Published => {
                if !standing.unreadable && standing.source_is_original && standing.backup.is_none() {
                    taken_back(abort(here, &mut record, Some(held), "it was cut short before the original was set aside"))
                } else {
                    drop(held);
                    Err(cannot_tell(&record))
                }
            }
            Phase::Swapping if standing.unreadable => {
                drop(held);
                Err(cannot_tell(&record))
            }
            Phase::Linked if standing.unreadable => {
                drop(held);
                Err(cannot_tell(&record))
            }
            Phase::Swapping => {
                if standing.source_is_original && standing.backup.is_none() {
                    taken_back(abort(here, &mut record, Some(held), "it was cut short before the original was set aside"))
                } else if standing.source_gone && standing.backup_is_original(&record) && standing.target_is_ours {
                    settle_names(&record)?;
                    link(here, &record)?;
                    forward(here, &mut record, held)
                } else if standing.source_is_link && standing.backup_is_original(&record) && standing.target_is_ours {
                    settle_names(&record)?;
                    forward(here, &mut record, held)
                } else {
                    drop(held);
                    Err(cannot_tell(&record))
                }
            }
            Phase::Linked => {
                if standing.source_is_link && standing.backup_is_original(&record) && standing.target_is_ours {
                    settle_names(&record)?;
                    forward(here, &mut record, held)
                } else {
                    drop(held);
                    Err(cannot_tell(&record))
                }
            }
            Phase::Done | Phase::Aborted => unreachable!("settled"),
        }
    }

    /// Puts on disk the directories that hold the move's names — the source
    /// and its backup, the target — as a rename cut short may have left them
    /// not yet there, before anything after it is kept.
    fn settle_names(record: &Record) -> anyhow::Result<()> {
        for path in [&record.source, &record.backup, &record.target] {
            let parent = path.parent().ok_or_else(|| anyhow!("{} has no directory", path.display()))?;
            fastctx_platform::durable_file::sync_directory(parent).with_context(|| format!("putting {} on disk", parent.display()))?;
        }
        Ok(())
    }

    /// Whether the login of `record` leads still, through this move's link and
    /// those of the moves after it, to the copy the last of them made.
    fn still_led(here: &Here, record: &Record) -> anyhow::Result<bool> {
        let finished = moves::finished(here.world.path())?;
        let (mut source, mut target, mut published) = (record.source.clone(), record.target.clone(), record.target_identity);
        for _ in 0..moves::MOST_MOVES {
            if fs::read_link(&source).ok().as_ref() != Some(&target) {
                return Ok(false);
            }
            let Ok(metadata) = fs::symlink_metadata(&target) else { return Ok(false) };
            if metadata.is_dir() {
                let real = fs::canonicalize(&record.login).ok();
                return Ok(Some(Identity { device: metadata.dev(), inode: metadata.ino() }) == published
                    && real.is_some()
                    && real == fs::canonicalize(&target).ok());
            }
            // Moved on since: the next move took this very directory.
            let Some(next) = finished.iter().find(|next| next.source == target && Some(next.source_identity) == published)
            else {
                return Ok(false);
            };
            (source, target, published) = (next.source.clone(), next.target.clone(), next.target_identity);
        }
        Ok(false)
    }

    fn describe(path: &Path) -> String {
        match fs::symlink_metadata(path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => "not there".into(),
            Err(error) => format!("unreadable ({error})"),
            Ok(metadata) if metadata.file_type().is_symlink() => {
                format!("a link to {}", fs::read_link(path).map(|led| led.display().to_string()).unwrap_or_else(|_| "?".into()))
            }
            Ok(metadata) if metadata.is_dir() => format!("a directory (device {}, inode {})", metadata.dev(), metadata.ino()),
            Ok(_) => "a file".into(),
        }
    }

    /// Deletes the original a finished move set aside: the same directory,
    /// nothing mounted in it, unused, and, unless `anyway`, as it was when it
    /// was copied.
    fn drop_backup(here: &Here, op: &str, anyway: bool) -> anyhow::Result<()> {
        let (_, mut record) = moves::records(here.world.path())?
            .into_iter()
            .find(|(_, record)| record.op == op)
            .ok_or_else(|| anyhow!("no move {op} is recorded"))?;
        if record.phase != Phase::Done {
            bail!("the move {op} is not done ({:?})", record.phase);
        }
        if record.backup_deleted {
            println!("the original of the move {op} was deleted already");
            return Ok(());
        }
        if !still_led(here, &record)? {
            bail!(
                "{} no longer leads to the copy the move made ({} is {}); the original set aside is not deleted",
                record.login.display(),
                record.source.display(),
                describe(&record.source)
            );
        }
        let metadata = fs::symlink_metadata(&record.backup).with_context(|| format!("looking at {}", record.backup.display()))?;
        if !metadata.is_dir() || (Identity { device: metadata.dev(), inode: metadata.ino() }) != record.source_identity {
            bail!("{} is not the original this move set aside; it is not deleted", record.backup.display());
        }
        if let Some(point) = mounted_in(&record.backup)?.first() {
            bail!("a filesystem is mounted at {}, in the original set aside; it is not deleted", point.display());
        }
        let found = occupants::of(&record.backup, &record.agent, false, here.owner);
        if !found.is_empty() {
            let said: Vec<String> = found.iter().map(|occupant| occupant.said()).collect();
            bail!("the original set aside is in use; it is not deleted:\n  {}", said.join("\n  "));
        }
        let copied = fs::read_to_string(manifest_path(here, op)).context("reading what was copied")?;
        let now = tree::manifest(&record.backup)?;
        if now != copied {
            let then: std::collections::BTreeSet<&str> = copied.lines().collect();
            let changed: Vec<&str> = now.lines().filter(|line| !then.contains(line)).take(20).collect();
            if !anyway {
                bail!(
                    "the original changed after it was copied — written to after the move? — so it is not deleted; these differ (and maybe more):\n  {}\n(--anyway deletes it all the same)",
                    changed.join("\n  ")
                );
            }
        }
        // Looked at again just before: nothing changed while it was gone through.
        if !still_led(here, &record)? {
            bail!("{} no longer leads to the copy the move made; the original set aside is not deleted", record.login.display());
        }
        let again = fs::symlink_metadata(&record.backup).with_context(|| format!("looking at {}", record.backup.display()))?;
        if !again.is_dir() || (Identity { device: again.dev(), inode: again.ino() }) != record.source_identity {
            bail!("{} is not the original this move set aside any more; it is not deleted", record.backup.display());
        }
        if let Some(point) = mounted_in(&record.backup)?.first() {
            bail!("a filesystem is mounted at {}, in the original set aside; it is not deleted", point.display());
        }
        fastctx_platform::owned_directory::delete(&record.backup).with_context(|| format!("deleting {}", record.backup.display()))?;
        record.backup_deleted = true;
        keep(here, &record)?;
        println!("deleted {}", record.backup.display());
        Ok(())
    }
}
