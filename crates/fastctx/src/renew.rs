//! Local renewal after an executable has arrived. Distribution is outside
//! this module. The worker runs as the user; only `guardian` requests elevation.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, bail, ensure};
use clap::{Args, Subcommand};
use fastctx_client::renewal::{self, Probe};
use fastctx_install::renewal::{self as store, Plan};
use fastctx_platform::FileLock;
use fastctx_types::layout::WorldDir;
use fastctx_types::renewal::{Action, Phase, Status};

#[derive(Args)]
pub struct RenewalArgs {
    #[command(subcommand)]
    action: RenewalAction,
}

#[derive(Subcommand)]
enum RenewalAction {
    /// Identify this executable without opening any World or host state.
    Identify,
    #[command(hide = true)]
    Notices,
    #[command(hide = true)]
    Prepare {
        #[arg(long, value_parser = crate::paths::parser())]
        dir: PathBuf,
        #[arg(long)]
        digest: String,
    },
    /// Put a local executable in place and request an idle handover.
    Stage {
        #[arg(long, value_parser = crate::paths::parser())]
        dir: PathBuf,
        #[arg(long, value_parser = crate::paths::parser(), required_unless_present = "package")]
        source: Option<PathBuf>,
        /// A release's command-line package instead, as a node's own update
        /// takes it: the program and its own signature come out of it.
        #[arg(long, value_parser = crate::paths::parser(), conflicts_with = "source")]
        package: Option<PathBuf>,
    },
    /// Check and repair this installation without changing user data.
    Repair(crate::repair::RepairArgs),
    /// Read the last repair, recognizing a repair process that has ended.
    RepairState {
        #[arg(long, value_parser = crate::paths::parser())]
        dir: PathBuf,
    },
    /// Inspect local versions and renewal progress, even across IPC versions.
    Status {
        #[arg(long, value_parser = crate::paths::parser())]
        dir: PathBuf,
    },
    /// Request progress now, still waiting for every active turn and call.
    Now {
        #[arg(long, value_parser = crate::paths::parser())]
        dir: PathBuf,
    },
    #[command(hide = true)]
    Run {
        #[arg(long, value_parser = crate::paths::parser())]
        dir: PathBuf,
        #[arg(long)]
        retry: bool,
    },
    /// Update the protected guardian, requesting administrator rights once:
    /// of this machine's World, or the one `--dir` names.
    Guardian {
        #[arg(long, value_parser = crate::paths::parser())]
        dir: Option<PathBuf>,
        #[arg(long, hide = true)]
        graphical: bool,
    },
    #[command(hide = true)]
    GuardianApply {
        #[arg(long, value_parser = crate::paths::parser())]
        dir: PathBuf,
        #[arg(long)]
        user_sid: Option<String>,
        #[arg(long)]
        uid: Option<u32>,
    },
}

pub fn run(args: RenewalArgs) -> anyhow::Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    match args.action {
        RenewalAction::Identify => println!("{}", serde_json::to_string(&store::build()?)?),
        RenewalAction::Notices => println!(
            "{}",
            serde_json::to_string(
                &fastctx_ripgrep::licenses::files().into_iter().collect::<std::collections::BTreeMap<_, _>>()
            )?
        ),
        RenewalAction::Prepare { dir, digest } => {
            runtime.block_on(fastctx_node::replacement::prepare(&dir, &digest))?
        }
        RenewalAction::Stage { dir, source, package } => {
            let plan = match (source, package) {
                (_, Some(package)) => {
                    let unpacking = std::env::temp_dir().join(format!("fastctx-package-{}", std::process::id()));
                    store::stage_package(&dir, &package, &unpacking)?
                }
                (Some(source), None) => store::stage(&dir, &source, None)?,
                (None, None) => bail!("give --source or --package"),
            };
            if plan.phase != Phase::Complete {
                store::request(&dir, true)?;
            }
        }
        RenewalAction::Status { dir } => println!("{}", serde_json::to_string(&runtime.block_on(status(&dir))?)?),
        RenewalAction::Repair(args) => crate::repair::run(args)?,
        RenewalAction::RepairState { dir } => println!("{}", serde_json::to_string(&fastctx_install::repair::inspect(&dir)?)?),
        RenewalAction::Now { dir } => store::request(&dir, true)?,
        RenewalAction::Run { dir, retry } => runtime.block_on(worker(&dir, retry))?,
        RenewalAction::Guardian { dir, graphical } => {
            let dir = match (dir, std::env::var_os("SUDO_USER")) {
                (Some(dir), _) => dir,
                // Under sudo the World found would be root's, never the one
                // waiting: the command asks for an administrator itself.
                (None, Some(user)) => bail!(
                    "run `fastctx renew guardian` as {}, without sudo: it asks for an administrator itself",
                    user.to_string_lossy()
                ),
                (None, None) => fastctx_platform::fastctx_root()?,
            };
            runtime.block_on(guardian(&dir, graphical))?
        }
        RenewalAction::GuardianApply { dir, user_sid, uid } => runtime.block_on(guardian_apply(&dir, user_sid, uid))?,
    }
    Ok(())
}

async fn status(root: &Path) -> anyhow::Result<Status> {
    store::status(root).await
}

async fn worker(root: &Path, retry: bool) -> anyhow::Result<()> {
    anyhow::ensure!(!root.join("uninstalled.json").try_exists()?, "this World is being uninstalled");
    let Some(_worker) = FileLock::try_acquire(&store::directory(root).join("worker.lock"))? else { return Ok(()) };
    let mut plan = store::read(root)?.context("no local replacement was staged")?;
    if plan.phase == Phase::Failed && !retry && !plan.retry.ready(jiff::Timestamp::now()) {
        return Ok(());
    }
    let result = advance(root, &mut plan).await;
    if let Err(error) = &result {
        plan.error = Some(format!("{error:#}"));
        plan.error_version = Some(plan.candidate.build.version.clone());
        plan.retry.postpone(jiff::Timestamp::now());
        plan.phase = Phase::Failed;
        store::save(root, &plan)?;
    }
    result
}

fn phase(root: &Path, plan: &mut Plan, phase: Phase) -> anyhow::Result<()> {
    plan.phase = phase;
    if plan.phase == Phase::Complete {
        plan.error = None;
        plan.error_version = None;
        plan.retry = Default::default();
    }
    store::save(root, plan)?;
    Ok(())
}

async fn complete(root: &Path, plan: &mut Plan) -> anyhow::Result<()> {
    let (folder, digest) = (root.to_path_buf(), plan.candidate.build.digest.clone());
    tokio::task::spawn_blocking(move || follow_hubs(&folder, &digest)).await??;
    phase(root, plan, Phase::Complete)
}

/// The boot service's side has not taken the new program yet: the renewal
/// waits in the service's step, saying why, and the node asks it again within
/// half a minute (`fastctx_node::renewal::resume`). Nobody is waited for
/// (`world-upgrade.md`, 换版不要人确认).
fn waiting_for_service(root: &Path, plan: &mut Plan, why: String) -> anyhow::Result<()> {
    plan.error = Some(why);
    phase(root, plan, Phase::UpdatingGuardian)
}

async fn advance(root: &Path, plan: &mut Plan) -> anyhow::Result<()> {
    let world = WorldDir::under(root);
    let kept = world.node_service().exists();
    let service =
        fastctx_install::service::Service::new(fastctx_install::service::Kind::Node, store::installed(root), root);
    let needs_guardian = kept && !service.guardian_current(&plan.candidate.build.digest)?;
    let current = renewal::probe(&world).await?;
    if let Probe::Running(node) = &current
        && node.build == plan.candidate.build
        && std::fs::canonicalize(&node.executable)? == std::fs::canonicalize(store::installed(root))?
    {
        if node.bridge {
            return finish_bridge(root, plan, needs_guardian).await;
        }
        if !needs_guardian && (!kept || node.supervised || cfg!(target_os = "linux")) {
            return complete(root, plan).await;
        }
        // The guardian runs the candidate as its node already, as after a
        // handover whose guardian refused the program: a bridge would wait
        // for that node to leave, which its guardian never lets it.
        if needs_guardian && node.supervised && cfg!(windows) {
            return guard_supervised(root, plan).await;
        }
    }
    phase(root, plan, Phase::Preparing)?;
    let prepared = store::directory(root).join(format!("prepared-{}.json", plan.candidate.build.digest));
    let existing = store::read_json::<fastctx_types::ProcessIdentity>(&prepared)?;
    let candidate = match existing {
        Some(identity) if fastctx_platform::is_alive(&identity)? => identity,
        _ => store::launch(
            root,
            &plan.candidate.path,
            &[
                "renew".into(),
                "prepare".into(),
                "--dir".into(),
                root.into(),
                "--digest".into(),
                plan.candidate.build.digest.clone().into(),
            ],
        )?
        .identity
        .context("replacement exited before it could prepare")?,
    };
    let began = std::time::Instant::now();
    loop {
        if !fastctx_platform::is_alive(&candidate)? {
            bail!("replacement exited before preparing; current node was left running");
        }
        if store::read_json::<fastctx_types::ProcessIdentity>(&prepared)?.as_ref() == Some(&candidate) {
            break;
        }
        // Preparing takes seconds; a replacement alive that never says it is
        // ready holds the renewal's gate for nothing, so it is ended and the
        // renewal tried again on its schedule.
        if began.elapsed() > STALLED {
            let _ = fastctx_platform::terminate::terminate(&candidate);
            bail!("the replacement did not get ready in {} minutes", STALLED.as_secs() / 60);
        }
        tick().await;
    }
    let legacy = matches!(current, Probe::Legacy { .. });
    if legacy {
        // 2026-10-04: the shipped HandOver converts a colleague's first
        // queued input into failure while draining. Its admission boundary
        // cannot be repaired by this client. A prepared node waits for the
        // old instance to leave naturally, then resumes this durable plan.
        return phase(root, plan, Phase::AwaitingLegacyExit);
    }
    phase(root, plan, Phase::WaitingForIdle)?;
    let mut asked = None;
    if !matches!(current, Probe::Absent | Probe::Legacy { .. }) {
        asked = settled(renewal::process(&world).await)?.flatten();
        settled(renewal::request(&world, Action::Yield).await)?;
    }
    loop {
        let observed = observing(&world).await?;
        if let Probe::Running(node) = &observed
            && node.build == plan.candidate.build
            && std::fs::canonicalize(&node.executable)? == std::fs::canonicalize(store::installed(root))?
            && asked.as_ref() != Some(&node.process)
        {
            return if node.bridge {
                finish_bridge(root, plan, needs_guardian).await
            } else {
                complete(root, plan).await
            };
        }
        if !fastctx_platform::is_alive(&candidate)? {
            if let Probe::Running(node) = &observed
                && node.build == plan.candidate.build
            {
                if !needs_guardian {
                    return complete(root, plan).await;
                }
                bail!("the prepared process stopped; the new node serves, but its guardian still needs renewal");
            }
            recover(root, plan, kept).await?;
            bail!("replacement failed before publication; the previous World program was retained");
        }
        if matches!(observed, Probe::Legacy { .. }) {
            return phase(root, plan, Phase::AwaitingLegacyExit);
        }
        let serving = settled(renewal::process(&world).await)?.flatten();
        if serving.is_some() && serving != asked && settled(renewal::request(&world, Action::Yield).await)?.is_some() {
            asked = serving;
        }
        tick().await;
    }
}

/// What a call to a node that may be leaving came to: its answer, or none
/// where the node went away as it was asked, to be looked at again.
fn settled<T>(result: std::io::Result<T>) -> std::io::Result<Option<T>> {
    match result {
        Ok(answer) => Ok(Some(answer)),
        Err(error) if transient(&error) => Ok(None),
        Err(error) => Err(error),
    }
}

async fn finish_bridge(root: &Path, plan: &mut Plan, needs_guardian: bool) -> anyhow::Result<()> {
    if !plan.keeper_restarted {
        let service =
            fastctx_install::service::Service::new(fastctx_install::service::Kind::Node, store::installed(root), root);
        // The protected guardian, SYSTEM's, takes an officially signed newer
        // program by itself (`world-upgrade.md`, 换版不要人确认); one that
        // cannot now, not running or of an earlier release, is asked again
        // until it does, its reason kept.
        let mut refused = None;
        let staged = !needs_guardian || service.guardian_staged(&plan.candidate.build.digest)? || {
            mend_signature(root, plan);
            let (asked, digest) = (service.clone(), plan.candidate.build.digest.clone());
            match tokio::task::spawn_blocking(move || asked.adopt_guardian(&digest)).await? {
                Ok(taken) => taken,
                Err(error) => {
                    refused = Some(format!("{error:#}"));
                    false
                }
            }
        };
        if !staged {
            let why = refused.unwrap_or_else(|| "the boot service has not taken the new program yet".to_string());
            return waiting_for_service(root, plan, why);
        }
        // A macOS node is the user's own LaunchAgent, restarted by its user.
        // A stopped Linux unit restarts as root, or by the rule its install
        // left (`fastctx_install::service::Service::may_restart`); where it
        // has none, its own checking timer, root's, starts it within a minute
        // on the program in place now, and the bridge serves till then.
        #[cfg(target_os = "linux")]
        let restart = {
            let status = service.status()?;
            service.main_pid()? == 0 && !status.state.starts_with("activating/") && service.may_restart().is_ok()
        };
        #[cfg(not(target_os = "linux"))]
        let restart = true;
        let _gate = FileLock::acquire(&store::directory(root).join("manager-gate.lock"))?;
        let Probe::Running(node) = renewal::probe(&WorldDir::under(root)).await? else {
            bail!("no serving replacement bridge")
        };
        ensure!(node.bridge && node.build == plan.candidate.build, "the replacement bridge is not serving");
        phase(root, plan, Phase::UpdatingGuardian)?;
        if restart {
            service.restart_guardian(&plan.candidate.build.digest)?;
        }
        plan.keeper_restarted = true;
        store::save(root, plan)?;
    }
    phase(root, plan, Phase::ReturningToService)?;
    let world = WorldDir::under(root);
    renewal::request(&world, Action::Yield).await?;
    let began = std::time::Instant::now();
    loop {
        returning(began)?;
        if let Probe::Running(node) = observing(&world).await?
            && !node.bridge
            && node.build.digest == plan.candidate.build.digest
        {
            #[cfg(target_os = "linux")]
            {
                let service = fastctx_install::service::Service::new(
                    fastctx_install::service::Kind::Node,
                    store::installed(root),
                    root,
                );
                ensure!(
                    service.main_pid()? == u64::from(node.process.pid),
                    "the replacement is not the service's node"
                );
            }
            return complete(root, plan).await;
        }
        tick().await;
    }
}

/// The guardian takes the program its node runs already, by the program's
/// own signature, and starts again on it; the node it starts is the
/// candidate, so the renewal is complete once it serves. A guardian that
/// will not yet is asked again (`waiting_for_service`), no bridge waiting.
async fn guard_supervised(root: &Path, plan: &mut Plan) -> anyhow::Result<()> {
    let service =
        fastctx_install::service::Service::new(fastctx_install::service::Kind::Node, store::installed(root), root);
    let digest = plan.candidate.build.digest.clone();
    let mut refused = None;
    let staged = service.guardian_staged(&digest)? || {
        mend_signature(root, plan);
        let (asked, digest) = (service.clone(), digest.clone());
        match tokio::task::spawn_blocking(move || asked.adopt_guardian(&digest)).await? {
            Ok(taken) => taken,
            Err(error) => {
                refused = Some(format!("{error:#}"));
                false
            }
        }
    };
    if !staged {
        let why = refused.unwrap_or_else(|| "the boot service has not taken the new program yet".to_string());
        return waiting_for_service(root, plan, why);
    }
    // The guardian starting again ends the node it runs: that node goes once
    // idle first, as at a handover, so no turn under way is cut off. The node
    // the guardian brings up meanwhile waits at the gate a service node takes
    // first (`fastctx_node::renewal`), held here till the guardian has
    // started again, so it takes no turn the restart would cut off.
    let gate = FileLock::acquire(&store::directory(root).join("manager-gate.lock"))?;
    phase(root, plan, Phase::WaitingForIdle)?;
    let world = WorldDir::under(root);
    if let Some(serving) = settled(renewal::process(&world).await)?.flatten() {
        settled(renewal::request(&world, Action::Yield).await)?;
        while let Probe::Running(node) = observing(&world).await?
            && node.process == serving
        {
            tick().await;
        }
    }
    phase(root, plan, Phase::UpdatingGuardian)?;
    let (restarting, digest) = (service.clone(), digest.clone());
    tokio::task::spawn_blocking(move || restarting.restart_guardian(&digest)).await??;
    drop(gate);
    plan.keeper_restarted = true;
    store::save(root, plan)?;
    let began = std::time::Instant::now();
    loop {
        returning(began)?;
        if let Probe::Running(node) = observing(&world).await?
            && !node.bridge
            && node.build.digest == plan.candidate.build.digest
        {
            return complete(root, plan).await;
        }
        tick().await;
    }
}

/// Mends the signature beside the installed program before its guardian is
/// asked to take it (`store::mend_signature`); a failure is the guardian's
/// to say.
fn mend_signature(root: &Path, plan: &Plan) {
    if cfg!(windows)
        && let Err(error) = store::mend_signature(root, &plan.candidate)
    {
        eprintln!("mending the program's signature: {error:#}");
    }
}

/// A hub this computer keeps runs the same World program and would go on
/// speaking the old network protocol (`world-upgrade.md`, 中枢跟着换). Once
/// the node is back in its boot service, the hub is started again on the new
/// program where its user may, and its machines reconnect by themselves: on
/// Windows its guardian, SYSTEM's, takes the signed program first. Where its
/// user may not, the hub follows by itself within a minute, its guardian
/// taking the newer signed program (`windows_guardian::adopt::offered`), or,
/// on Linux and macOS, the hub ending once it sees the program it runs from
/// replaced by a newer one, for its service to start it again
/// (`fastctx_hub::renewal`).
fn follow_hubs(root: &Path, digest: &str) -> anyhow::Result<()> {
    for directory in fastctx_install::service::registered_hubs(root)? {
        let hub = fastctx_install::service::Service::new(
            fastctx_install::service::Kind::Hub,
            store::installed(root),
            &directory,
        );
        // Where this cannot, the hub follows by itself; why it could not is
        // still said, for whoever reads the renewal's log.
        if let Err(error) = follow_hub(&hub, digest) {
            eprintln!("the hub at {} follows the new program by itself: {error:#}", directory.display());
        }
    }
    Ok(())
}

/// Starts the hub `hub` again on the program `digest`, when it is not on it
/// and its user may.
fn follow_hub(hub: &fastctx_install::service::Service, digest: &str) -> anyhow::Result<()> {
    if !hub.status()?.installed {
        return Ok(());
    }
    if cfg!(windows) {
        if hub.guardian_current(digest)? || !hub.adopt_guardian(digest)? {
            return Ok(());
        }
    } else {
        hub.may_restart()?;
    }
    hub.restart_guardian(digest)
}

async fn recover(root: &Path, plan: &Plan, kept: bool) -> anyhow::Result<()> {
    let world = WorldDir::under(root);
    let previous = plan.previous.as_ref().context("the first World program failed to start")?;
    ensure!(
        fastctx_install::program::inspect(&store::installed(root))? == previous.build,
        "replacement failed after publication; the newer stable World program is retained, and no older image is installed"
    );
    loop {
        if let Probe::Running(node) = observing(&world).await? {
            if !node.yielding {
                return Ok(());
            }
            match renewal::request(&world, Action::Cancel).await {
                Ok(()) => return Ok(()),
                Err(error) if error.to_string() == "handover already committed" || transient(&error) => {
                    // Commit may have won. The writer lock below decides
                    // when restarting is valid; a failed cancel is no proof.
                }
                Err(error) => return Err(error.into()),
            }
        }
        if FileLock::try_acquire(&world.node_lock())?.is_some() {
            if !kept {
                ensure!(plan.previous.is_some(), "no previous executable is available for recovery");
                store::launch(root, &store::installed(root), &["node".into(), "--dir".into(), root.into()])?;
                loop {
                    if !matches!(observing(&world).await?, Probe::Absent) {
                        return Ok(());
                    }
                    tick().await;
                }
            }
            return Ok(());
        }
        tick().await;
    }
}

pub(super) async fn observing(world: &WorldDir) -> std::io::Result<Probe> {
    let result = tokio::time::timeout(std::time::Duration::from_secs(30), renewal::probe(world)).await
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, format!("checking node at {} exceeded 30 seconds", world.path().display())))?;
    match result {
        Err(error) if transient(&error) => Ok(Probe::Absent),
        result => result,
    }
}

/// A node going away as it is asked: gone, refusing, closing the
/// connection, or, on macOS, closed before the connection was looked at.
pub(super) fn transient(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::NotFound
            | std::io::ErrorKind::ConnectionRefused
            | std::io::ErrorKind::UnexpectedEof
            | std::io::ErrorKind::BrokenPipe
            | std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::NotConnected
    )
}

async fn guardian(root: &Path, graphical: bool) -> anyhow::Result<()> {
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let _ = graphical;
    let plan = store::read(root)?.context("no local replacement was staged")?;
    ensure!(
        matches!(plan.phase, Phase::AwaitingAdministrator | Phase::UpdatingGuardian | Phase::Failed),
        "the replacement has not reached guardian renewal"
    );
    store::verify(&plan.candidate)?;
    let Probe::Running(node) = renewal::probe(&WorldDir::under(root)).await? else {
        bail!("the replacement bridge is not serving yet")
    };
    // A guardian that runs the candidate as its node already takes its own
    // image as well, as one behind a bridge does.
    ensure!(
        (node.bridge || node.supervised) && node.build == plan.candidate.build,
        "the replacement bridge is not serving yet"
    );
    let service =
        fastctx_install::service::Service::new(fastctx_install::service::Kind::Node, store::installed(root), root);
    if cfg!(windows) && service.guardian_staged(&plan.candidate.build.digest)? {
        return store::request(root, true);
    }
    #[cfg(windows)]
    {
        let sid = fastctx_platform::current_user_sid()?;
        let args =
            ["renew".into(), "guardian-apply".into(), "--dir".into(), root.into(), "--user-sid".into(), sid.into()];
        let code = fastctx_platform::elevation::run_elevated(&plan.candidate.path, &args)?;
        ensure!(code == 0, "guardian replacement failed (exit {code}); renewal status has the result");
        store::request(root, true)?;
        Ok(())
    }
    #[cfg(target_os = "linux")]
    {
        let mut plan = plan;
        let uid = std::fs::metadata(root).map(|metadata| {
            use std::os::unix::fs::MetadataExt as _;
            metadata.uid()
        })?;
        let status = std::process::Command::new(if graphical { "pkexec" } else { "sudo" })
            .arg(store::installed(root))
            .args(["renew", "guardian-apply", "--dir"])
            .arg(root)
            .arg("--uid")
            .arg(uid.to_string())
            .status()?;
        ensure!(status.success(), "system-service handover failed: {status}");
        plan.keeper_restarted = true;
        store::save(root, &plan)?;
        store::request(root, true)
    }
    #[cfg(target_os = "macos")]
    {
        let mut plan = plan;
        let uid = std::fs::metadata(root).map(|metadata| {
            use std::os::unix::fs::MetadataExt as _;
            metadata.uid()
        })?;
        let args: Vec<std::ffi::OsString> = vec![
            "renew".into(),
            "guardian-apply".into(),
            "--dir".into(),
            root.into(),
            "--uid".into(),
            uid.to_string().into(),
        ];
        let installed = store::installed(root);
        let code = if graphical {
            fastctx_platform::elevation::run_elevated(&installed, &args)?
        } else {
            let status = std::process::Command::new("sudo").arg(&installed).args(&args).status()?;
            u32::from(!status.success())
        };
        ensure!(code == 0, "system-service handover failed (exit {code})");
        plan.keeper_restarted = true;
        store::save(root, &plan)?;
        store::request(root, true)
    }
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    bail!("this platform has no boot-service handover")
}

async fn guardian_apply(root: &Path, user_sid: Option<String>, uid: Option<u32>) -> anyhow::Result<()> {
    ensure!(fastctx_platform::elevation::is_elevated()?, "guardian replacement requires administrator rights");
    let own = store::build()?;
    let service = {
        #[allow(unused_mut)]
        let mut service =
            fastctx_install::service::Service::new(fastctx_install::service::Kind::Node, store::installed(root), root);
        #[cfg(windows)]
        {
            service.user_sid = user_sid;
        }
        #[cfg(not(windows))]
        let _ = user_sid;
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            service.uid = uid.context("system-service handover needs its user UID")?;
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        let _ = uid;
        service
    };
    // This process may belong to a different approving administrator. It
    // never opens the user's private World, locks, or IPC; it only stages
    // its own approved image in the existing protected registration.
    #[cfg(windows)]
    {
        service.renew_guardian(&own.digest)?;
        // A hub this computer keeps runs the same World program and would
        // otherwise go on speaking the old network protocol. It has no
        // bridge to stand in for it, so it takes the new image and restarts
        // now; its machines reconnect by themselves.
        for directory in fastctx_install::service::registered_hubs(root)? {
            let hub = fastctx_install::service::Service::new(
                fastctx_install::service::Kind::Hub,
                store::installed(root),
                &directory,
            );
            if hub.status()?.installed && !hub.guardian_current(&own.digest)? {
                hub.renew_guardian(&own.digest)?;
                hub.restart_guardian(&own.digest)?;
            }
        }
        Ok(())
    }
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        service.restart_guardian(&own.digest)?;
        // A unit an earlier release installed has no rule letting its user
        // restart it; this approval leaves the one an install now does, so
        // it is the last a renewal here asks for.
        if let Err(error) = service.allow_restart(service.uid) {
            eprintln!("the boot service's user may not restart it by itself yet: {error:#}");
        }
        // systemd and launchd run a kept hub straight from the World
        // program, so a restart is all it takes to run the new one.
        for directory in fastctx_install::service::registered_hubs(root)? {
            let hub = fastctx_install::service::Service::new(
                fastctx_install::service::Kind::Hub,
                store::installed(root),
                &directory,
            );
            if hub.status()?.installed {
                hub.restart_guardian(&own.digest)?;
                if let Err(error) = hub.allow_restart(service.uid) {
                    eprintln!("the hub's user may not restart it by itself yet: {error:#}");
                }
            }
        }
        Ok(())
    }
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    bail!("this platform has no boot-service handover")
}

/// How long a step that takes seconds or a minute is waited for before the
/// renewal takes it for stalled: what holds its gate is let go, the failure
/// kept with why, and the renewal tried again on its schedule (`world-upgrade.md`,
/// 没成的，自己再试). A turn under way is never this: it is waited for.
const STALLED: std::time::Duration = std::time::Duration::from_secs(10 * 60);

/// Fails a return to the boot service that has not come about in
/// `STALLED`; the next try finds the node as it is then and goes on.
fn returning(began: std::time::Instant) -> anyhow::Result<()> {
    ensure!(
        began.elapsed() <= STALLED,
        "the boot service did not start the new program as its node in {} minutes",
        STALLED.as_secs() / 60
    );
    Ok(())
}

async fn tick() {
    // Same process-start observation interval as Client::connect_or_start.
    // There is no handover deadline and no busy process is killed.
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
}
