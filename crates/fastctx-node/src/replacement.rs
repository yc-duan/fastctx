//! A prepared delivery coordinates publication; only the stable program serves.

use std::path::Path;
use std::time::{Duration, Instant};

const STALLED: Duration = Duration::from_secs(600);

use anyhow::{Context as _, ensure};
use fastctx_install::renewal as store;
use fastctx_platform::{FileLock, current_identity, ensure_private_dir, is_alive};
use fastctx_types::{ProcessIdentity, layout::WorldDir};

fn witness(root: &Path, digest: &str) -> std::path::PathBuf {
    store::coordinator(root, digest)
}

/// A kept bridge retains the node startup gate until its stable child yields.
/// The delivery never
/// opens the node listener or writes World records as a serving node.
pub async fn prepare(root: &Path, digest: &str) -> anyhow::Result<()> {
    let root = root.to_owned();
    let digest = digest.to_owned();
    tokio::task::spawn_blocking(move || {
        let build = store::build()?;
        ensure!(build.digest == digest, "prepared delivery differs from its plan");
        let node_entry = fastctx_install::placement::node_activation(&root)?;
        let identity = current_identity()?;
        store::write_json(&witness(&root, &digest), &identity)?;
        store::write_json(&store::directory(&root).join(format!("prepared-{digest}.json")), &identity)?;
        let world = WorldDir::under(&root);
        ensure_private_dir(world.path())?;
        let _service = if world.node_service().exists() {
            Some(FileLock::acquire(&store::directory(&root).join("service-gate.lock"))?)
        } else {
            None
        };
        let writer = FileLock::acquire(&world.node_lock())?;
        let activation = fastctx_install::placement::activation(&root)?;
        let plan = store::read(&root)?.context("replacement has no plan")?;
        ensure!(plan.candidate.build == build, "replacement plan changed before publication");
        store::install_plan(&root, &plan)?;
        let child = store::launch(
            &root,
            &store::installed(&root),
            &[
                "node".into(),
                "--dir".into(),
                root.as_os_str().to_owned(),
                "--successor".into(),
                digest.clone().into(),
                "--prepared-by".into(),
                serde_json::to_string(&identity)?.into(),
            ],
        )?
        .identity
        .context("stable replacement exited during launch")?;
        drop(writer);
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
        let began = Instant::now();
        loop {
            ensure!(is_alive(&child)?, "the stable replacement exited before serving");
            ensure!(began.elapsed() < STALLED, "the stable replacement did not serve within ten minutes; its program was retained");
            let remaining = STALLED.saturating_sub(began.elapsed());
            let observed = runtime.block_on(async {
                tokio::time::timeout(remaining.min(Duration::from_secs(30)), fastctx_client::renewal::probe(&world)).await
            });
            if let Ok(Ok(fastctx_client::renewal::Probe::Running(node))) = &observed
                && node.process == child
                && node.build == build
            {
                break;
            }
            if let Ok(Err(error)) = observed
                && !matches!(error.kind(), std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
                    | std::io::ErrorKind::UnexpectedEof | std::io::ErrorKind::BrokenPipe
                    | std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::ConnectionAborted
                    | std::io::ErrorKind::NotConnected)
            {
                return Err(error.into());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        drop(activation);
        if _service.is_none() {
            drop(node_entry);
            return Ok(());
        }
        let abandoned = Instant::now();
        while is_alive(&child)? {
            // An active renewal may be waiting for a user's turn. Only a
            // coordinator left without its worker expires on its own.
            if abandoned.elapsed() >= STALLED
                && let Some(_worker) = FileLock::try_acquire(&store::directory(&root).join("worker.lock"))?
            {
                if let Some(mut plan) = store::read(&root)?
                    && plan.candidate.build == build
                {
                    plan.phase = fastctx_types::renewal::Phase::Failed;
                    plan.error = Some("the handover coordinator waited ten minutes without a renewal worker; the serving node was left running".into());
                    plan.error_version = Some(build.version.clone());
                    plan.retry.postpone(jiff::Timestamp::now());
                    store::save(&root, &plan)?;
                }
                break;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        Ok(())
    })
    .await?
}

pub(crate) fn validate(root: &Path, digest: &str, parent: &ProcessIdentity) -> anyhow::Result<()> {
    ensure!(
        store::read_json::<ProcessIdentity>(&witness(root, digest))?.as_ref() == Some(parent),
        "replacement coordinator does not match"
    );
    ensure!(is_alive(parent)?, "replacement coordinator has exited");
    let plan = store::read(root)?.context("replacement has no plan")?;
    ensure!(plan.candidate.build == store::build()?, "stable replacement differs from its plan");
    ensure!(
        std::fs::canonicalize(std::env::current_exe()?)? == std::fs::canonicalize(store::installed(root))?,
        "replacement must start from the stable World program"
    );
    Ok(())
}
