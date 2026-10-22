//! Restoring an owned boot registration without interrupting active turns.

use anyhow::{Context as _, bail};
use fastctx_client::renewal::{self, Probe};
use fastctx_install::{renewal as store, repair::Journal};
use fastctx_types::{layout::WorldDir, renewal::Build};
use std::path::Path;
use std::time::Duration;

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
                while matches!(runtime.block_on(crate::renew::observing(&world))?, Probe::Running(current) if current.process == node.process)
                {
                    std::thread::sleep(Duration::from_millis(250));
                }
            }
            Probe::Legacy { .. } => bail!("legacy node must exit before its service can be repaired"),
            Probe::Absent => {}
        }
    }
    if !fastctx_platform::elevation::is_elevated()? && !cfg!(target_os = "macos") {
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::fs::MetadataExt as _;
            let uid = std::fs::metadata(root)?.uid();
            let mut command = std::process::Command::new("sudo");
            command
                .arg("-n")
                .arg(std::env::current_exe()?)
                .args(["service", "node", "install", "--dir"])
                .arg(root)
                .arg("--executable")
                .arg(store::installed(root))
                .arg("--uid")
                .arg(uid.to_string());
            let output = fastctx_platform::bounded::output(
                command,
                "restoring boot service without a prompt",
                Duration::from_secs(120),
            )?;
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
