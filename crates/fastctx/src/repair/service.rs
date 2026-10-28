//! Restoring an owned boot registration without interrupting active turns.

use anyhow::{Context as _, bail};
use fastctx_client::renewal::{self, Probe};
use fastctx_install::{renewal as store, repair::Journal};
use fastctx_types::{layout::WorldDir, renewal::Build};
use std::path::Path;
use std::time::Duration;

/// How long a repair waits for the serving node to hand the World over.
const YIELDING: Duration = Duration::from_secs(30 * 60);

pub(super) fn run(
    root: &Path,
    build: &Build,
    runtime: &tokio::runtime::Runtime,
    journal: &mut Journal,
) -> anyhow::Result<String> {
    let world = WorldDir::under(root);
    if !super::connected(root)? && !world.node_service().try_exists()? {
        return Ok("no network requires a boot service".into());
    }
    let service =
        fastctx_install::service::Service::new(fastctx_install::service::Kind::Node, store::installed(root), root);
    let mut waiting = std::time::Instant::now();
    while fastctx_platform::FileLock::try_acquire(&store::directory(root).join("worker.lock"))?.is_none() {
        if store::read(root)?.is_some_and(|p| p.phase == fastctx_types::renewal::Phase::WaitingForIdle) {
            waiting = std::time::Instant::now();
        }
        anyhow::ensure!(
            waiting.elapsed() < Duration::from_secs(600),
            "renewal worker for {} did not finish its service step in ten minutes",
            root.display()
        );
        std::thread::sleep(Duration::from_millis(250));
    }
    let status = service.status();
    let definition = service.definition_current();
    let guardian = service.guardian_current(&build.digest);
    if let Ok(status) = &status
        && status.installed
        && status.enabled
        && (status.state == "running" || status.state == "active/running")
        && status
            .recovery
            .as_ref()
            .is_none_or(|r| r.enabled && (r.state == "active" || r.state == "ready" || r.state == "running"))
        && guardian.as_ref().is_ok_and(|v| *v)
        && definition.as_ref().is_ok_and(|v| *v)
    {
        if !world.node_service().try_exists()? {
            journal.action(format!("restore service marker {}", world.node_service().display()))?;
            std::fs::write(world.node_service(), b"")?;
        }
        return Ok(format!("{} is enabled and running", status.name));
    }
    let description = match &status {
        Ok(status) => format!("{} (state {}, enabled {})", status.name, status.state, status.enabled),
        Err(error) => format!("registration for {} (inspection failed: {error:#})", root.display()),
    };
    let mut adopted = false;
    if cfg!(windows)
        && definition.as_ref().is_ok_and(|v| *v)
        && status.as_ref().is_ok_and(|s| {
            s.installed && s.enabled && s.state == "running" && s.recovery.as_ref().is_some_and(|r| r.enabled)
        })
        && !guardian.as_ref().is_ok_and(|v| *v)
    {
        journal.action(format!("ask the signed guardian to adopt {}", build.version))?;
        match service.adopt_guardian(&build.digest) {
            Ok(taken) => adopted = taken,
            Err(error) => journal.action(format!("guardian adoption did not complete: {error:#}"))?,
        }
    }
    let disruptive = !definition.as_ref().is_ok_and(|v| *v) || !guardian.as_ref().is_ok_and(|v| *v);
    // Installation verifies ownership again and recovers unfinished manager
    // transactions. An inspection failure is retained, never called absent.
    journal.action(format!("restore boot service {description}; definition {definition:?}; guardian {guardian:?}"))?;
    let _manager = fastctx_platform::FileLock::acquire(&store::directory(root).join("manager-gate.lock"))?;
    if disruptive {
        match runtime.block_on(crate::renew::observing(&world))? {
            Probe::Running(node) => {
                if let Err(error) = runtime
                    .block_on(async {
                        tokio::time::timeout(
                            Duration::from_secs(30),
                            renewal::request(&world, fastctx_types::renewal::Action::Yield),
                        )
                        .await
                    })
                    .context("requesting idle service handover exceeded 30 seconds")?
                    && !crate::renew::transient(&error)
                {
                    return Err(error.into());
                }
                // The node hands over once no turn is running; a repair waits
                // for that as long as an update does, and then says so.
                let asked = std::time::Instant::now();
                while matches!(runtime.block_on(crate::renew::observing(&world))?, Probe::Running(current) if current.process == node.process)
                {
                    anyhow::ensure!(
                        asked.elapsed() < YIELDING,
                        "the node serving {} did not hand over within {} minutes; turns may still be running",
                        root.display(),
                        YIELDING.as_secs() / 60
                    );
                    std::thread::sleep(Duration::from_millis(250));
                }
            }
            Probe::Legacy { .. } => bail!("legacy node must exit before its service can be repaired"),
            Probe::Absent => {}
        }
    }
    if adopted {
        service
            .restart_guardian(&build.digest)
            .with_context(|| format!("restarting adopted guardian for {}", root.display()))?;
        return Ok("signed guardian adopted the program and restarted".into());
    }
    if !fastctx_platform::elevation::is_elevated()? && !cfg!(target_os = "macos") {
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::fs::MetadataExt as _;
            let uid = std::fs::metadata(root)?.uid();
            let output = crate::renew::install_unasked(root, &std::env::current_exe()?)?;
            anyhow::ensure!(
                output.status.success(),
                "repairing {description} requires: sudo {} service node install --dir {} --executable {} --uid {uid}; {}",
                fastctx_platform::shell_quote::path(&std::env::current_exe()?),
                fastctx_platform::shell_quote::path(root),
                fastctx_platform::shell_quote::path(&store::installed(root)),
                String::from_utf8_lossy(&output.stderr)
            );
            return Ok(format!("restored {description}"));
        }
        #[cfg(not(target_os = "linux"))]
        bail!("repairing {description} requires administrator rights; no confirmation was opened");
    }
    service.install().with_context(|| format!("repairing {description}"))?;
    Ok(format!("restored {description}"))
}
