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
mod linux {
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

    /// This machine's fastctx: its root and World, the user who owns them,
    /// and the service that keeps its node, where one does.
    struct Here {
        root: PathBuf,
        world: WorldDir,
        owner: u32,
        home: PathBuf,
        service: Option<Service>,
    }

    impl Here {
        fn find() -> anyhow::Result<Self> {
            let root = fastctx_platform::fastctx_root()?;
            let world = WorldDir::under(&root);
            let owner = fs::metadata(&root).with_context(|| format!("reading {}", root.display()))?.uid();
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
            Ok(Self { root, world, owner, home, service })
        }

        fn records(&self) -> PathBuf {
            self.world.path().join(moves::DIRECTORY)
        }
    }

    /// Refuses to run from inside this machine's fastctx — a conversation's
    /// shell, a command its node started — which stopping the node would stop
    /// with it (the user, 2026-11-21: "这里是不是控制端和被控端是一个，被控端
    /// 停止了，控制端是不是也停了"): it is run in a login of this machine's
    /// own, over ssh.
    fn outside_fastctx() -> anyhow::Result<()> {
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
        if !args.status {
            outside_fastctx()?;
        }
        fastctx_platform::ensure_private_dir(&here.records())?;
        let _alone = FileLock::try_acquire(&here.records().join(".lock"))?
            .ok_or_else(|| anyhow!("another `fastctx move-login` runs on this World"))?;
        if args.status {
            for (path, record) in moves::records(here.world.path())? {
                println!(
                    "{}  {:?}  {} -> {}{}{}  ({})",
                    record.op,
                    record.phase,
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
            return Ok(());
        }
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
        let pending = moves::records(here.world.path())?
            .into_iter()
            .find(|(_, record)| !record.phase.settled() || (record.phase == Phase::Done && !record.service_restored));
        if let Some((path, _)) = pending {
            bail!("a move is unfinished ({}); finish it with `fastctx move-login --resume`", path.display());
        }
        let moved = start(&here, &login, &target, args.unmounted_at_boot_ok, args.stop_agents);
        if moved.is_err() && unfinished(&here) {
            eprintln!("fastctx: the move is unfinished; `fastctx move-login --resume` finishes it");
        }
        moved
    }

    /// Whether a move is left to finish, or its service to put back.
    fn unfinished(here: &Here) -> bool {
        moves::records(here.world.path()).map_or(true, |records| {
            records.iter().any(|(_, record)| !record.phase.settled() || (record.phase == Phase::Done && !record.service_restored))
        })
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

    fn keep(here: &Here, record: &Record) -> anyhow::Result<()> {
        moves::keep(&here.records().join(format!("{}.json", record.op)), record)
            .with_context(|| format!("keeping the record of the move {}", record.op))?;
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
    struct Held {
        _node: FileLock,
        _apply: FileLock,
    }

    /// Stops the node's service, where one keeps it, and takes the node's
    /// lock and the apply lock, waiting a while for each.
    fn hold(here: &Here) -> anyhow::Result<Held> {
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
        if let (Some(service), Some(before)) = (&here.service, &record.service) {
            if before.enabled {
                service.release().context("starting the node's service again")?;
            } else {
                println!("the node's service was held before the move; it is left held");
            }
        }
        record.service_restored = true;
        keep(here, record)
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
        let left = fastctx_install::apply::moved::unfinished(&here.root)?;
        if !left.is_empty() {
            bail!("an apply was left unfinished ({}); run `fastctx apply` to finish it first", left.join(", "));
        }
        fastctx_install::apply::moved::conflicts(&here.root, &source, &target)?;
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
        println!("moving {} ({agent}) to {}: the node is stopped until it is done (move {op})", login.display(), target.display());
        let held = match hold(here) {
            Ok(held) => held,
            Err(error) => return abort(here, &mut record, None, &format!("{error:#}")),
        };
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
            fastctx_platform::publish(&manifest_path(here, &op), copied.manifest.as_bytes(), fastctx_platform::Permissions::Private)?;
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
        if let Err(error) = tree::rename_new(&source, &record.backup) {
            return abort(here, &mut record, Some(held), &format!("setting {} aside: {error}", source.display()));
        }
        link(&record)?;
        forward(here, &mut record, held)
    }

    /// Puts the link to the target where the source was: made beside it,
    /// then renamed into its place, which must be empty.
    fn link(record: &Record) -> anyhow::Result<()> {
        let beside = record.source.with_file_name(format!(".fastctx-link-{}", record.op));
        match fs::symlink_metadata(&beside) {
            Ok(metadata) if metadata.file_type().is_symlink() => fs::remove_file(&beside)?,
            Ok(_) => bail!("{} is there and is no link of this move's", beside.display()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        std::os::unix::fs::symlink(&record.target, &beside).with_context(|| format!("making the link {}", beside.display()))?;
        tree::rename_new(&beside, &record.source).with_context(|| format!("putting the link at {}", record.source.display()))?;
        Ok(())
    }

    /// From the link on: the receipts follow the directory, the marker goes,
    /// the service is put back.
    fn forward(here: &Here, record: &mut Record, held: Held) -> anyhow::Result<()> {
        record.phase = Phase::Linked;
        keep(here, record)?;
        for done in fastctx_install::apply::moved::moved(&here.root, &record.source, &record.target)? {
            println!("{done}");
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
            if stop_agents && rounds < 3 && found.iter().any(|occupant| occupant.agent) {
                rounds += 1;
                for occupant in found.iter().filter(|occupant| occupant.agent) {
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
        bail!("the login was not moved: {}", record.why.as_deref().unwrap_or(why))
    }

    /// Takes away the staging directory and the published copy this move
    /// made, the original untouched.
    fn undo(record: &Record) -> anyhow::Result<()> {
        if fs::symlink_metadata(&record.staging).is_ok() {
            if !tree::staged_by(&record.staging, &record.op) {
                bail!("{} is not this move's; it is left", record.staging.display());
            }
            fastctx_platform::owned_directory::delete(&record.staging)?;
        }
        if let Ok(found) = identity(&record.target)
            && (Some(found) == record.target_identity || tree::staged_by(&record.target, &record.op))
            && fs::symlink_metadata(&record.target).is_ok_and(|metadata| metadata.is_dir())
        {
            fastctx_platform::owned_directory::delete(&record.target)?;
        }
        Ok(())
    }

    /// Finishes the move cut short, from where it stands on disk.
    fn resume(here: &Here) -> anyhow::Result<()> {
        let found = moves::records(here.world.path())?
            .into_iter()
            .find(|(_, record)| !record.phase.settled() || (record.phase == Phase::Done && !record.service_restored));
        let Some((_, mut record)) = found else {
            println!("no move is unfinished");
            return Ok(());
        };
        println!("finishing the move {} of {} to {} ({:?})", record.op, record.login.display(), record.target.display(), record.phase);
        if record.phase == Phase::Done {
            unmark(&record)?;
            return restore(here, &mut record, None);
        }
        let held = hold(here)?;
        // Taken back, the move is finished as well as it can be: said, not failed.
        let taken_back = |result: anyhow::Result<()>| {
            if let Err(error) = result {
                println!("{error:#}");
            }
            Ok(())
        };
        match record.phase {
            Phase::Stopping | Phase::Checking | Phase::Copying | Phase::Verifying | Phase::Published => {
                taken_back(abort(here, &mut record, Some(held), "it was cut short before the original was set aside"))
            }
            Phase::Swapping => {
                let source = fs::symlink_metadata(&record.source);
                let backup = identity(&record.backup).ok();
                let target = identity(&record.target).ok();
                let target_is_ours = target.is_some() && target == record.target_identity;
                let source_is_original = source.as_ref().is_ok_and(|metadata| {
                    metadata.is_dir() && Identity { device: metadata.dev(), inode: metadata.ino() } == record.source_identity
                });
                let source_is_link = source.as_ref().is_ok_and(|metadata| metadata.file_type().is_symlink())
                    && fs::read_link(&record.source).is_ok_and(|led| led == record.target);
                let source_gone = matches!(&source, Err(error) if error.kind() == std::io::ErrorKind::NotFound);
                let backup_is_original = backup == Some(record.source_identity);
                if source_is_original && backup.is_none() {
                    taken_back(abort(here, &mut record, Some(held), "it was cut short before the original was set aside"))
                } else if source_gone && backup_is_original && target_is_ours {
                    link(&record)?;
                    forward(here, &mut record, held)
                } else if source_is_link && backup_is_original && target_is_ours {
                    forward(here, &mut record, held)
                } else {
                    let why = format!(
                        "the move cannot tell how to go on: {} is {}, {} is {}, {} is {}; nothing is changed — look, put things right, and run --resume again",
                        record.source.display(),
                        describe(&record.source),
                        record.backup.display(),
                        describe(&record.backup),
                        record.target.display(),
                        describe(&record.target)
                    );
                    drop(held);
                    bail!(why)
                }
            }
            Phase::Linked => {
                let target = identity(&record.target).ok();
                if target.is_none() || target != record.target_identity {
                    drop(held);
                    bail!("{} is not the copy this move made ({}); nothing is changed", record.target.display(), describe(&record.target));
                }
                forward(here, &mut record, held)
            }
            Phase::Done | Phase::Aborted => unreachable!("settled"),
        }
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
    /// unused, and, unless `anyway`, as it was when it was copied.
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
        let metadata = fs::symlink_metadata(&record.backup).with_context(|| format!("looking at {}", record.backup.display()))?;
        if !metadata.is_dir() || (Identity { device: metadata.dev(), inode: metadata.ino() }) != record.source_identity {
            bail!("{} is not the original this move set aside; it is not deleted", record.backup.display());
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
        fastctx_platform::owned_directory::delete(&record.backup).with_context(|| format!("deleting {}", record.backup.display()))?;
        record.backup_deleted = true;
        keep(here, &record)?;
        println!("deleted {}", record.backup.display());
        Ok(())
    }
}
