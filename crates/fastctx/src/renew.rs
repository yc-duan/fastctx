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
    /// Put a local executable in place and request an idle handover.
    Stage {
        #[arg(long)]
        dir: PathBuf,
        #[arg(long)]
        source: PathBuf,
    },
    /// Inspect local versions and renewal progress, even across IPC versions.
    Status {
        #[arg(long)]
        dir: PathBuf,
    },
    /// Request progress now, still waiting for every active turn and call.
    Now {
        #[arg(long)]
        dir: PathBuf,
    },
    #[command(hide = true)]
    Run {
        #[arg(long)]
        dir: PathBuf,
        #[arg(long)]
        retry: bool,
    },
    /// Update the protected guardian, requesting administrator rights once.
    Guardian {
        #[arg(long)]
        dir: PathBuf,
    },
    #[command(hide = true)]
    GuardianApply {
        #[arg(long)]
        dir: PathBuf,
        #[arg(long)]
        user_sid: Option<String>,
    },
}

pub fn run(args: RenewalArgs) -> anyhow::Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    match args.action {
        RenewalAction::Identify => println!("{}", serde_json::to_string(&store::build()?)?),
        RenewalAction::Stage { dir, source } => {
            let running = runtime.block_on(renewal::image(&WorldDir::under(&dir)))?;
            store::stage(&dir, &source, running.as_deref())?;
            store::request(&dir, true)?;
        }
        RenewalAction::Status { dir } => println!("{}", serde_json::to_string(&runtime.block_on(status(&dir))?)?),
        RenewalAction::Now { dir } => store::request(&dir, true)?,
        RenewalAction::Run { dir, retry } => runtime.block_on(worker(&dir, retry))?,
        RenewalAction::Guardian { dir } => runtime.block_on(guardian(&dir))?,
        RenewalAction::GuardianApply { dir, user_sid } => runtime.block_on(guardian_apply(&dir, user_sid))?,
    }
    Ok(())
}

async fn status(root: &Path) -> anyhow::Result<Status> {
    let plan = store::read(root)?;
    let (node, mismatch) = match observing(&WorldDir::under(root)).await? {
        Probe::Running(node) => (Some(node), None),
        Probe::Legacy { mismatch, .. } => (None, Some(mismatch)),
        Probe::Absent => (None, None),
    };
    let phase = plan.as_ref().map(|plan| {
        if plan.phase == Phase::Complete
            && (mismatch.is_some() || node.as_ref().is_some_and(|node| node.build != plan.candidate.build))
        {
            Phase::Staged
        } else {
            plan.phase.clone()
        }
    });
    Ok(Status {
        node,
        mismatch,
        pending: plan.as_ref().filter(|_| phase != Some(Phase::Complete)).map(|plan| plan.candidate.build.clone()),
        phase,
        error: plan.and_then(|plan| plan.error),
    })
}

async fn worker(root: &Path, retry: bool) -> anyhow::Result<()> {
    let Some(_worker) = FileLock::try_acquire(&store::directory(root).join("worker.lock"))? else { return Ok(()) };
    let mut plan = store::read(root)?.context("no local replacement was staged")?;
    if plan.phase == Phase::Failed && !retry {
        return Ok(());
    }
    let mut result = advance(root, &mut plan).await;
    if result.is_err()
        && plan.phase == Phase::Preparing
        && let Some(previous) = &plan.previous
        && let Err(error) = store::install(root, previous)
    {
        result = Err(anyhow::anyhow!("{}; restoring the previous image also failed: {error}", result.unwrap_err()));
    }
    if let Err(error) = &result {
        plan.error = Some(format!("{error:#}"));
        plan.phase = Phase::Failed;
        store::save(root, &plan)?;
    }
    result
}

fn phase(root: &Path, plan: &mut Plan, phase: Phase) -> anyhow::Result<()> {
    plan.phase = phase;
    plan.error = None;
    store::save(root, plan)?;
    Ok(())
}

async fn advance(root: &Path, plan: &mut Plan) -> anyhow::Result<()> {
    store::install(root, &plan.candidate)?;
    let world = WorldDir::under(root);
    let kept = world.node_service().exists();
    let service =
        fastctx_install::service::Service::new(fastctx_install::service::Kind::Node, store::installed(root), root);
    let needs_guardian = kept && !service.guardian_current(&plan.candidate.build.digest)?;
    let current = renewal::probe(&world).await?;
    if let Probe::Running(node) = &current
        && node.build.digest == plan.candidate.build.digest
    {
        if node.bridge {
            return finish_bridge(root, plan, needs_guardian).await;
        }
        if !needs_guardian {
            return phase(root, plan, Phase::Complete);
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
                "node".into(),
                "--dir".into(),
                root.into(),
                "--successor".into(),
                plan.candidate.build.digest.clone().into(),
            ],
        )?
        .identity
        .context("replacement exited before it could prepare")?,
    };
    loop {
        if !fastctx_platform::is_alive(&candidate)? {
            if let Some(previous) = &plan.previous {
                store::install(root, previous)?;
            }
            bail!("replacement exited before preparing; current node was left running");
        }
        if store::read_json::<fastctx_types::ProcessIdentity>(&prepared)?.as_ref() == Some(&candidate) {
            break;
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
        asked = renewal::process(&world).await?;
        renewal::request(&world, Action::Yield).await?;
    }
    loop {
        let observed = observing(&world).await?;
        if let Probe::Running(node) = &observed
            && node.process == candidate
        {
            return if node.bridge {
                finish_bridge(root, plan, needs_guardian).await
            } else {
                phase(root, plan, Phase::Complete)
            };
        }
        if !fastctx_platform::is_alive(&candidate)? {
            if let Probe::Running(node) = &observed
                && node.build == plan.candidate.build
            {
                if !needs_guardian {
                    return phase(root, plan, Phase::Complete);
                }
                bail!("the prepared process stopped; the new node serves, but its guardian still needs renewal");
            }
            recover(root, plan, kept).await?;
            bail!("replacement failed before serving; the previous image was restored when available");
        }
        if matches!(observed, Probe::Legacy { .. }) {
            return phase(root, plan, Phase::AwaitingLegacyExit);
        }
        let serving = renewal::process(&world).await?;
        if serving.is_some() && serving != asked {
            renewal::request(&world, Action::Yield).await?;
            asked = serving;
        }
        tick().await;
    }
}

async fn finish_bridge(root: &Path, plan: &mut Plan, needs_guardian: bool) -> anyhow::Result<()> {
    if needs_guardian {
        let service =
            fastctx_install::service::Service::new(fastctx_install::service::Kind::Node, store::installed(root), root);
        if !service.guardian_staged(&plan.candidate.build.digest)? {
            return phase(root, plan, Phase::AwaitingAdministrator);
        }
        let _gate = FileLock::acquire(&store::directory(root).join("manager-gate.lock"))?;
        let Probe::Running(node) = renewal::probe(&WorldDir::under(root)).await? else {
            bail!("no serving replacement bridge")
        };
        ensure!(node.bridge && node.build == plan.candidate.build, "the replacement bridge is not serving");
        phase(root, plan, Phase::UpdatingGuardian)?;
        service.restart_guardian(&plan.candidate.build.digest)?;
    }
    phase(root, plan, Phase::ReturningToService)?;
    let world = WorldDir::under(root);
    renewal::request(&world, Action::Yield).await?;
    loop {
        if let Probe::Running(node) = observing(&world).await?
            && !node.bridge
            && node.build.digest == plan.candidate.build.digest
        {
            return phase(root, plan, Phase::Complete);
        }
        tick().await;
    }
}

async fn recover(root: &Path, plan: &Plan, kept: bool) -> anyhow::Result<()> {
    let world = WorldDir::under(root);
    if let Some(previous) = &plan.previous {
        store::install(root, previous)?;
    }
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
                let previous = plan.previous.as_ref().context("no previous executable is available for recovery")?;
                store::launch(root, &previous.path, &["node".into(), "--dir".into(), root.into()])?;
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

async fn observing(world: &WorldDir) -> std::io::Result<Probe> {
    match renewal::probe(world).await {
        Err(error) if transient(&error) => Ok(Probe::Absent),
        result => result,
    }
}

fn transient(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::NotFound
            | std::io::ErrorKind::ConnectionRefused
            | std::io::ErrorKind::UnexpectedEof
            | std::io::ErrorKind::BrokenPipe
            | std::io::ErrorKind::ConnectionReset
    )
}

async fn guardian(root: &Path) -> anyhow::Result<()> {
    let plan = store::read(root)?.context("no local replacement was staged")?;
    ensure!(
        plan.phase == Phase::AwaitingAdministrator || plan.phase == Phase::Failed,
        "the replacement has not reached guardian renewal"
    );
    store::verify(&plan.candidate)?;
    let Probe::Running(node) = renewal::probe(&WorldDir::under(root)).await? else {
        bail!("the replacement bridge is not serving yet")
    };
    ensure!(node.bridge && node.build == plan.candidate.build, "the replacement bridge is not serving yet");
    let service =
        fastctx_install::service::Service::new(fastctx_install::service::Kind::Node, store::installed(root), root);
    if service.guardian_staged(&plan.candidate.build.digest)? {
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
    #[cfg(not(windows))]
    bail!("this platform has no bundled SYSTEM guardian to replace")
}

async fn guardian_apply(root: &Path, user_sid: Option<String>) -> anyhow::Result<()> {
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
        service
    };
    // This process may belong to a different approving administrator. It
    // never opens the user's private World, locks, or IPC; it only stages
    // its own approved image in the existing protected registration.
    service.renew_guardian(&own.digest)
}

async fn tick() {
    // Same process-start observation interval as Client::connect_or_start.
    // There is no handover deadline and no busy process is killed.
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
}
