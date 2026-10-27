//! Durable repair acceptance. Acquiring a package may use this node, but the
//! actual repair runs detached and does not hold a call across node handover.

use crate::Node;
use fastctx_install::renewal::{read_json, write_json};
use fastctx_types::{
    repair::{Outcome, Report},
    update::{Fetching, Stage, release},
};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, atomic::Ordering};

#[derive(Clone, Serialize, Deserialize)]
struct Pending {
    id: String,
    version: String,
    by: String,
    delay: fastctx_update::retry::Retry,
    observed: Option<jiff::Timestamp>,
    #[serde(default)]
    process: Option<fastctx_types::ProcessIdentity>,
    #[serde(default)]
    program: Option<std::path::PathBuf>,
    #[serde(default)]
    origin: Option<fastctx_net::ids::HubId>,
    #[serde(default)]
    cancelled: bool,
}

fn path(node: &Node) -> std::path::PathBuf {
    super::dir(node).join("repair-request.json")
}

pub(super) fn snapshot(node: &Node) -> Option<Report> {
    match fastctx_install::repair::read(&node.root) {
        Ok(report) => report,
        Err(error) => {
            node.log.line(format_args!("reading {}: {error}", super::dir(node).join("repair.json").display()));
            None
        }
    }
}

/// Explicit requests bypass the delay, but never overwrite a running repair.
pub(super) fn accept(node: &Arc<Node>, version: Option<String>, id: Option<String>, by: String) -> Result<(), String> {
    accept_from(node, version, id, by, None)
}

fn accept_from(
    node: &Arc<Node>,
    version: Option<String>,
    id: Option<String>,
    by: String,
    origin: Option<fastctx_net::ids::HubId>,
) -> Result<(), String> {
    fastctx_platform::ensure_private_dir(&super::dir(node)).map_err(|e| e.to_string())?;
    let version =
        fastctx_install::repair::target::select(&node.root, env!("CARGO_PKG_VERSION"), version.as_deref(), None)
            .map_err(|e| format!("selecting repair target: {e:#}"))?;
    let pending = Pending {
        id: id.unwrap_or_else(|| format!("{}-{}", jiff::Timestamp::now().as_nanosecond(), std::process::id())),
        version,
        by,
        delay: Default::default(),
        observed: None,
        process: None,
        program: None,
        origin,
        cancelled: false,
    };
    let _gate = fastctx_platform::FileLock::try_acquire(&super::dir(node).join("repair-request.lock"))
        .map_err(|e| e.to_string())?
        .ok_or("a repair request is being recorded")?;
    if snapshot(node).is_some_and(|r| r.id == pending.id && r.finished.is_some() && r.success) {
        return Ok(());
    }
    if let Some(existing) = read_json::<Pending>(&path(node)).map_err(|e| e.to_string())? {
        if existing.id == pending.id {
            return Ok(());
        }
        if !cleanup(node, &existing)? {
            return Err("a repair is already running".into());
        }
    }
    if node.updates.repair_launching.load(Ordering::Acquire) {
        return Err("a repair is already being prepared".into());
    }
    if let Some(report) = snapshot(node)
        && report.finished.is_none()
        && fastctx_platform::is_alive(&report.process).map_err(|e| e.to_string())?
    {
        return Err("a repair is already running".into());
    }
    write_json(&path(node), &pending).map_err(|e| format!("saving {}: {e}", path(node).display()))?;
    launch(node, pending);
    drop(_gate);
    Ok(())
}

fn log_progress(node: &Node, report: &Report) {
    let mark = serde_json::to_string(report).unwrap_or_default();
    let mut previous = node.updates.repair_logged.lock().expect("repair progress log");
    if *previous == mark {
        return;
    }
    let earlier = serde_json::from_str::<Report>(&previous).ok().filter(|r| r.id == report.id);
    *previous = mark;
    for step in &report.steps {
        if step.outcome != Outcome::Checking && !earlier.as_ref().is_some_and(|r| r.steps.contains(step)) {
            node.log
                .line(format_args!("repair {} {}: {:?}: {}", report.version, step.layer, step.outcome, step.detail));
        }
    }
}

/// Runs before ordinary update selection. A failed update's next attempt is a
/// repair even when automatic discovery is off, including an application or a
/// same-version renewal failure.
pub(super) fn drive(node: &Arc<Node>) -> bool {
    if node.updates.repair_launching.load(Ordering::Acquire) {
        return true;
    }
    match drive_checked(node) {
        Ok(busy) => busy,
        Err(error) => {
            node.log.line(format_args!("resuming installation repair at {}: {error}", path(node).display()));
            true
        }
    }
}

fn drive_checked(node: &Arc<Node>) -> Result<bool, String> {
    fastctx_platform::ensure_private_dir(&super::dir(node)).map_err(|e| e.to_string())?;
    let _gate = fastctx_platform::FileLock::try_acquire(&super::dir(node).join("repair-request.lock"))
        .map_err(|e| e.to_string())?;
    if _gate.is_none() {
        return Ok(true);
    }
    let report = fastctx_install::repair::read(&node.root).map_err(|e| e.to_string())?;
    if let Some(report) = &report {
        log_progress(node, report);
    }
    let mut pending = read_json::<Pending>(&path(node)).map_err(|e| e.to_string())?;
    if let Some(request) = pending.as_mut() {
        if let Some(origin) = &request.origin
            && let Some(link) = node.links().iter().find(|link| link.hub() == origin).cloned()
            && let Some(network) = link.current_versions()
            && network.repairs.get(&link.name()).is_none_or(|d| d.id != request.id)
            && !request.cancelled
            && !report.as_ref().is_some_and(|r| r.id == request.id && r.success)
        {
            request.cancelled = true;
            write_json(&path(node), request).map_err(|e| e.to_string())?;
        }
        if request.cancelled {
            if !cleanup(node, request)? {
                return Ok(true);
            }
            return Ok(!newer_requested(node, &request.version));
        }
    }
    if let Some(report) = &report
        && report.finished.is_none()
    {
        let owner =
            pending.as_ref().filter(|p| p.id == report.id).and_then(|p| p.process.as_ref()).unwrap_or(&report.process);
        let orphaned_preparation = owner == &report.process
            && report.process == fastctx_platform::current_identity().map_err(|e| e.to_string())?;
        if !orphaned_preparation && fastctx_platform::is_alive(owner).map_err(|e| e.to_string())? {
            return Ok(true);
        }
        let Some(_repair) = fastctx_platform::FileLock::try_acquire(&super::dir(node).join("repair.lock"))
            .map_err(|e| e.to_string())?
        else {
            return Ok(true);
        };
        let current = fastctx_install::repair::read(&node.root).map_err(|e| e.to_string())?;
        if current.as_ref().is_none_or(|r| {
            r.id != report.id || r.started != report.started || r.process != report.process || r.finished.is_some()
        }) {
            return Ok(true);
        }
        let mut ended = report.clone();
        ended.finished = Some(jiff::Timestamp::now());
        ended.error = Some(format!("repair process {} ended before recording completion", report.process.pid));
        ended.steps.push(fastctx_types::repair::Step {
            layer: "controller".into(),
            outcome: Outcome::Failed,
            detail: ended.error.clone().unwrap_or_default(),
            actions: Vec::new(),
        });
        write_json(&super::dir(node).join("repair.json"), &ended).map_err(|e| e.to_string())?;
        return Ok(true);
    }
    if let Some(report) = &report
        && report.finished.is_some()
    {
        if let Some(request) = pending.as_mut().filter(|p| p.id == report.id) {
            if report.success {
                if !cleanup(node, request)? {
                    return Ok(true);
                }
                std::fs::remove_file(path(node)).map_err(|e| e.to_string())?;
                if let Some(app) =
                    read_json::<super::AppFile>(&super::dir(node).join("app.json")).map_err(|e| e.to_string())?
                {
                    let version = app.app.version.clone();
                    {
                        let mut held = node.updates.held.lock().expect("repaired application");
                        held.app = Some(app.app);
                        held.program = app.program;
                        held.carried = app.carried;
                        held.app_executable = app.executable;
                    }
                    super::application::took(node, &version);
                }
                super::retry::complete(node, &report.version);
                return Ok(false);
            }
            if request.observed != Some(report.started) {
                if !cleanup(node, request)? {
                    return Ok(true);
                }
                request.observed = Some(report.started);
                request.process = None;
                request.delay.postpone(jiff::Timestamp::now());
                write_json(&path(node), request).map_err(|e| e.to_string())?;
            }
        } else if pending.is_none() && !report.success {
            let mut delay = fastctx_update::retry::Retry::default();
            delay.postpone(report.finished.unwrap_or(report.started));
            pending = Some(Pending {
                id: report.id.clone(),
                version: report.version.clone(),
                by: report.by.clone(),
                delay,
                observed: Some(report.started),
                process: None,
                program: None,
                origin: None,
                cancelled: false,
            });
            write_json(&path(node), pending.as_ref().ok_or("repair request disappeared")?)
                .map_err(|e| e.to_string())?;
        }
    }
    if pending.is_none() {
        let attempt = {
            let held = node.updates.held.lock().expect("update attempt");
            held.attempt
                .failed
                .as_ref()
                .filter(|f| {
                    !matches!(
                        f.reason,
                        Some(
                            fastctx_types::update::FailureReason::NoPackage
                                | fastctx_types::update::FailureReason::NotNewer
                        )
                    )
                })
                .map(|f| (f.version.clone(), held.attempt.delay.clone()))
        };
        let failed_plan = fastctx_install::renewal::read(&node.root)
            .map_err(|e| e.to_string())?
            .filter(|p| p.phase == fastctx_types::renewal::Phase::Failed)
            .map(|p| (p.candidate.build.version, p.retry));
        if let Some((version, delay)) = attempt.or(failed_plan) {
            if release(&version) < release(env!("CARGO_PKG_VERSION")) {
                return Ok(false);
            }
            pending = Some(Pending {
                id: format!("retry-{}-{}", jiff::Timestamp::now().as_nanosecond(), std::process::id()),
                version,
                by: "automatic retry".into(),
                delay,
                observed: None,
                process: None,
                program: None,
                origin: None,
                cancelled: false,
            });
            write_json(&path(node), pending.as_ref().ok_or("repair request disappeared")?)
                .map_err(|e| e.to_string())?;
        }
    }
    let Some(mut pending) = pending else { return Ok(false) };
    if !pending.delay.ready(jiff::Timestamp::now()) {
        return Ok(!newer_requested(node, &pending.version));
    }
    if pending.process.as_ref().is_some_and(|p| fastctx_platform::is_alive(p).unwrap_or(true)) {
        return Ok(true);
    }
    if node.updates.held.lock().expect("update activity").fetching.is_some() || super::application::busy() {
        return Ok(true);
    }
    let target =
        fastctx_install::repair::target::select(&node.root, env!("CARGO_PKG_VERSION"), Some(&pending.version), None)
            .map_err(|e| format!("selecting retry target: {e:#}"))?;
    if target != pending.version {
        pending.version = target;
        write_json(&path(node), &pending).map_err(|e| e.to_string())?;
    }
    launch(node, pending);
    drop(_gate);
    Ok(true)
}

fn launch(node: &Arc<Node>, pending: Pending) {
    if node.updates.repair_launching.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_err() {
        return;
    }
    let node = node.clone();
    tokio::spawn(async move {
        let outcome = prepare(&node, &pending).await;
        if let Err(why) = outcome {
            let result = (|| -> anyhow::Result<()> {
                let Some(_gate) = fastctx_platform::FileLock::try_acquire(&super::dir(&node).join("repair.lock"))?
                else {
                    return Ok(());
                };
                if let Some(receipt) = fastctx_install::repair::read(&node.root)?
                    && (receipt.id != pending.id
                        || receipt.finished.is_some()
                        || (receipt.process != fastctx_platform::current_identity()?
                            && fastctx_platform::is_alive(&receipt.process)?))
                {
                    return Ok(());
                }
                let mut journal = fastctx_install::repair::Journal::start(
                    &node.root,
                    &pending.version,
                    &pending.by,
                    Some(pending.id.clone()),
                )?;
                journal.begin("package")?;
                journal.end(Err(anyhow::anyhow!("{why}")))?;
                journal.finish()
            })();
            node.log.line(format_args!("preparing repair {}: {why}", pending.version));
            if let Err(error) = result {
                node.log.line(format_args!("saving failed repair: {error:#}"));
            }
        }
        {
            let mut held = node.updates.held.lock().expect("update activity");
            held.fetching = None;
            held.activity = None;
        }
        node.updates.repair_launching.store(false, Ordering::Release);
        super::report(&node);
    });
}

async fn prepare(node: &Arc<Node>, pending: &Pending) -> Result<(), String> {
    let _repair = fastctx_platform::FileLock::try_acquire(&super::dir(node).join("repair.lock"))
        .map_err(|e| e.to_string())?
        .ok_or("a repair is already running")?;
    {
        let mut journal = fastctx_install::repair::Journal::start(
            &node.root,
            &pending.version,
            &pending.by,
            Some(pending.id.clone()),
        )
        .map_err(|e| format!("{e:#}"))?;
        journal.begin("package").map_err(|e| format!("{e:#}"))?;
    }
    let manifest = super::manifest_of(node, &pending.version).await?;
    let _lease = node.updates.packages.retain(&pending.version);
    let mut targets = vec![fastctx_update::kernel::target()];
    if let Some(app) = &node.updates.held.lock().expect("application identity").app {
        targets.push(app.target.clone());
    }
    let mut archive = None;
    for target in targets {
        node.updates.packages.clear_incomplete(&super::cache(node), &pending.version, &target, |path| {
            fastctx_install::repair::note(
                &node.root,
                &pending.id,
                format!("delete stale package partial {}: no acquisition owns it", path.display()),
            )
            .map_err(|e| format!("{e:#}"))
        })?;
        let package =
            manifest.package(&target).ok_or_else(|| format!("{} has no package for {target}", manifest.version))?;
        if let Some(path) = super::cache(node).get(&pending.version, &target) {
            let (checked, signature, version) = (path.clone(), package.signature.clone(), pending.version.clone());
            let valid =
                tokio::task::spawn_blocking(move || fastctx_update::verify::file(&checked, &signature, &version))
                    .await
                    .map_err(|e| e.to_string())?;
            if let Err(why) = valid {
                fastctx_install::repair::note(
                    &node.root,
                    &pending.id,
                    format!("delete invalid package {}: {why}", path.display()),
                )
                .map_err(|e| format!("{e:#}"))?;
                node.log.line(format_args!("repair: deleting invalid package {}: {why}", path.display()));
                std::fs::remove_file(&path).map_err(|e| format!("deleting {}: {e}", path.display()))?;
            }
        }
        {
            let mut held = node.updates.held.lock().expect("repair package progress");
            held.fetching = Some(Fetching {
                version: pending.version.clone(),
                target: target.clone(),
                via: fastctx_types::update::Via::Github,
                received: 0,
                total: package.size,
            });
            held.activity = Some(Stage::Repairing { version: pending.version.clone(), layer: "package".into() });
        }
        if super::cache(node).get(&pending.version, &target).is_none() {
            fastctx_install::repair::note(
                &node.root,
                &pending.id,
                format!("obtain and verify {} for {target}", pending.version),
            )
            .map_err(|e| format!("{e:#}"))?;
        }
        super::report(node);
        let path = super::bring(node, &manifest, &target).await.map_err(|e| e.why)?;
        fastctx_update::verify::file(&path, &package.signature, &manifest.version)?;
        if target == fastctx_update::kernel::target() {
            archive = Some(path);
        }
    }
    if let Some(origin) = &pending.origin
        && let Some(link) = node.links().iter().find(|link| link.hub() == origin).cloned()
        && let Some(network) = link.current_versions()
        && network.repairs.get(&link.name()).is_none_or(|due| due.id != pending.id)
    {
        return Err("repair arrangement was withdrawn before execution".into());
    }
    let archive = archive.ok_or("repair acquired no command-line package")?;
    let root = node.root.clone();
    let pending = pending.clone();
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let directory = root.join("updates/repair-program").join(format!(
            "{}-{}",
            std::process::id(),
            jiff::Timestamp::now().as_nanosecond()
        ));
        std::fs::create_dir_all(&directory).map_err(|e| format!("creating {}: {e}", directory.display()))?;
        let program = fastctx_update::kernel::unpack(&archive, &directory)?;
        let build = fastctx_install::program::inspect(&program).map_err(|e| format!("{e:#}"))?;
        if build.version != pending.version {
            return Err(format!("repair program reports {}, expected {}", build.version, pending.version));
        }
        let process = fastctx_install::renewal::launch(
            &root,
            &program,
            &[
                "renew".into(),
                "repair".into(),
                "--dir".into(),
                root.as_os_str().into(),
                "--version".into(),
                pending.version.clone().into(),
                "--by".into(),
                pending.by.clone().into(),
                "--id".into(),
                pending.id.clone().into(),
                "--package".into(),
                archive.as_os_str().into(),
                "--accepted".into(),
            ],
        )
        .map_err(|e| format!("starting repair {}: {e}", program.display()))?
        .identity
        .ok_or("repair process exited during launch")?;
        let mut receipt =
            fastctx_install::repair::read(&root).map_err(|e| e.to_string())?.ok_or("repair acceptance disappeared")?;
        if receipt.id != pending.id {
            return Err("repair acceptance changed before launch".into());
        }
        receipt.process = process.clone();
        write_json(&root.join("updates/repair.json"), &receipt)
            .map_err(|e| format!("transferring repair ownership: {e}"))?;
        let mut request = pending;
        request.process = Some(process);
        request.program = Some(program);
        write_json(&root.join("updates/repair-request.json"), &request)
            .map_err(|e| format!("saving launched repair: {e}"))?;
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}

fn cleanup(node: &Node, request: &Pending) -> Result<bool, String> {
    if let Some(process) = &request.process
        && fastctx_platform::is_alive(process).map_err(|e| e.to_string())?
    {
        return Ok(false);
    }
    if let Some(program) = &request.program {
        let directory = program.parent().ok_or("repair program has no directory")?;
        if directory.parent() != Some(super::dir(node).join("repair-program").as_path()) {
            return Err("repair program is outside the owned staging directory".into());
        }
        match std::fs::remove_dir_all(directory) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("deleting {}: {error}", directory.display())),
        }
    }
    Ok(true)
}

/// Only the currently authenticated network notice creates a local arrangement.
pub(super) fn scheduled(node: &Arc<Node>) {
    for link in node.links() {
        let Some(network) = link.current_versions() else { continue };
        let Some(due) = network.repairs.get(&link.name()) else { continue };
        let result = accept_from(
            node,
            Some(due.version.clone()),
            Some(due.id.clone()),
            due.by.to_string(),
            Some(link.hub().clone()),
        );
        if let Err(why) = result
            && !why.contains("already")
            && !why.contains("being recorded")
        {
            node.log.line(format_args!("accepting repair {} from the hub: {why}", due.id));
        }
    }
}

pub(super) fn retry_at(node: &Node) -> Option<jiff::Timestamp> {
    read_json::<Pending>(&path(node)).ok().flatten().filter(|p| !p.cancelled).and_then(|p| p.delay.next)
}

fn newer_requested(node: &Node, version: &str) -> bool {
    let state = super::state(node);
    state.automatic.as_ref().filter(|_| state.auto).is_some_and(|v| release(v) > release(version))
        || node.links().iter().any(|link| {
            link.versions()
                .is_some_and(|n| n.due.get(&link.name()).is_some_and(|d| release(&d.version) > release(version)))
        })
}
