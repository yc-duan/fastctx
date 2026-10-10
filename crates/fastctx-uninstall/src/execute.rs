use crate::{Action, Request, Step};
use anyhow::Context as _;
use fastctx_install::{apply, uninstall};
use fastctx_types::uninstall::{Event, Left, Result as Outcome};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Default, Serialize, Deserialize)]
struct Progress {
    outcomes: BTreeMap<String, (Outcome, Option<String>)>,
    retained: Vec<Left>,
}

fn publish(path: &Path, value: &impl Serialize) -> anyhow::Result<()> {
    let bytes = serde_json::to_vec(value)?;
    match std::fs::read(path) {
        Ok(before) if before == bytes => return Ok(()),
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    fastctx_platform::publish(path, &bytes, fastctx_platform::Permissions::Private)?;
    Ok(())
}

pub async fn run(request: Request, mut events: impl FnMut(Event)) -> anyhow::Result<()> {
    request.installation.validate()?;
    let root = &request.installation.root;
    let world = fastctx_types::layout::WorldDir::under(root);
    let places = request.installation.places(std::env::current_exe()?);
    if !root.try_exists()? && request.steps.iter().all(|step| matches!(step.action, Action::Path | Action::Programs)) {
        for step in &request.steps {
            events(step_event(step, Ok(())));
        }
        events(Event::Finished { left: Vec::new() });
        return Ok(());
    }
    fastctx_platform::ensure_private_dir(root)?;
    let lock = fastctx_platform::uninstall_gate::Gate::try_acquire(root)?
        .context("another uninstall is executing for this World")?;
    if !request.installation.marker().try_exists()? {
        publish(&request.installation.marker(), &request)?;
    }
    let progress_file = root.join("uninstall-progress.json");
    let mut progress = crate::read::<Progress>(&progress_file)?.unwrap_or_default();
    if !request.plan.everything {
        let integrations = request
            .steps
            .iter()
            .filter_map(|step| if let Action::Host(home) = &step.action { Some(home.clone()) } else { None })
            .collect::<Vec<_>>();
        apply::remember_withdrawn(&places, &integrations)?;
    }
    let admin = request.steps.iter().filter(|step| step.item.admin).cloned().collect::<Vec<_>>();
    for event in request.system_results.clone() {
        record_event(event, &mut progress, &progress_file, &mut events)?;
    }
    let mut system = if admin.is_empty() || !request.system_results.is_empty() {
        None
    } else {
        match crate::privileged::Session::begin(&request, &mut |event| {
            record_event(event, &mut progress, &progress_file, &mut events)
        })
        .await
        {
            Ok(system) => Some(system),
            Err(error) => {
                for step in &admin {
                    record(step, Err(anyhow::anyhow!("{error:#}")), &mut progress, &progress_file, &mut events)?;
                }
                None
            }
        }
    };
    let mut node = None;
    let mut running_stopped = true;
    for step in request.steps.iter().filter(|step| matches!(step.action, Action::Stop | Action::Logout { .. })) {
        if !matches!(step.action, Action::Stop)
            && progress.outcomes.get(&step.item.id).is_some_and(|(outcome, _)| *outcome == Outcome::Done)
        {
            events(step_event(step, Ok(())));
            continue;
        }
        let result = match &step.action {
            Action::Stop => match crate::runtime::stop(&request.installation).await {
                Ok(identity) => {
                    node = identity;
                    Ok(())
                }
                Err(error) => {
                    running_stopped = false;
                    Err(error)
                }
            },
            Action::Logout { identity, directory, account } if running_stopped => {
                crate::logout::logout(&fastctx_types::layout::WorldDir::at(identity), directory, account.as_ref()).await
            }
            Action::Logout { .. } => {
                Err(anyhow::anyhow!("the node could not be disconnected before contacting its hubs"))
            }
            _ => unreachable!(),
        };
        record(step, result, &mut progress, &progress_file, &mut events)?;
    }
    if let Some(system) = system.as_mut()
        && let Err(error) =
            system.finish_system(&mut |event| record_event(event, &mut progress, &progress_file, &mut events)).await
    {
        for step in &admin {
            if !progress.outcomes.get(&step.item.id).is_some_and(|(outcome, _)| *outcome == Outcome::Done) {
                record(step, Err(anyhow::anyhow!("{error:#}")), &mut progress, &progress_file, &mut events)?;
            }
        }
    }
    let system_removed = admin
        .iter()
        .all(|step| progress.outcomes.get(&step.item.id).is_some_and(|(outcome, _)| *outcome == Outcome::Done));
    if running_stopped && system_removed {
        uninstall::delete_file(&world.node_service())?;
        if let Err(error) = crate::runtime::finish(node.as_ref()).await {
            running_stopped = false;
            progress.retained.push(Left {
                kind: "runtime".into(),
                what: "World node".into(),
                location: node.as_ref().map(|identity| identity.pid.to_string()),
                why: format!("{error:#}"),
            });
        }
    }
    for step in request
        .steps
        .iter()
        .filter(|step| !step.item.admin && !matches!(step.action, Action::Stop | Action::Logout { .. }))
    {
        if progress.outcomes.get(&step.item.id).is_some_and(|(outcome, _)| *outcome == Outcome::Done) {
            events(step_event(step, Ok(())));
            continue;
        }
        let hosts_withdrawn = request
            .steps
            .iter()
            .filter(|step| matches!(step.action, Action::Host(_)))
            .all(|step| progress.outcomes.get(&step.item.id).is_some_and(|(outcome, _)| *outcome == Outcome::Done));
        let result = if !running_stopped || !system_removed {
            Err(anyhow::anyhow!(
                "the World still has running processes or boot registrations; dependent cleanup was not performed"
            ))
        } else if matches!(step.action, Action::Programs | Action::Delete(_) | Action::WorldData) && !hosts_withdrawn {
            Err(anyhow::anyhow!(
                "a host integration could not be withdrawn; its restoration records and program assets must remain for retry"
            ))
        } else {
            perform(&request, step, &mut progress).await
        };
        record(step, result, &mut progress, &progress_file, &mut events)?;
    }
    progress.retained.sort_by(|left, right| {
        (&left.kind, &left.location, &left.why).cmp(&(&right.kind, &right.location, &right.why))
    });
    progress.retained.dedup();
    let mut left = remaining(&request, &progress);
    let hosts_withdrawn = request
        .steps
        .iter()
        .filter(|step| matches!(step.action, Action::Host(_)))
        .all(|step| progress.outcomes.get(&step.item.id).is_some_and(|(outcome, _)| *outcome == Outcome::Done));
    if hosts_withdrawn {
        for (object, error) in fastctx_install::transaction::prune_owned(root)? {
            left.push(Left {
                kind: "runtime".into(),
                what: "An earlier fastctx program or record kept by a host publication".into(),
                location: Some(object.display().to_string()),
                why: format!("It could not be removed: {error}"),
            });
        }
    }
    // The versions of the user's files are named once, by the archive that
    // holds them; one kept elsewhere, beside a file on another volume, by
    // its own place.
    let history = root.join("apply-history");
    let mut archived = false;
    for archive in fastctx_install::transaction::retained_archives(root)? {
        let location = if archive.starts_with(&history) {
            if std::mem::replace(&mut archived, true) {
                continue;
            }
            history.clone()
        } else {
            archive
        };
        let entry = Left { kind: "user-files".into(), what: "User file versions retained by host publication transactions".into(), location: Some(location.display().to_string()), why: "These original file objects can receive later user writes through old handles; they cannot be discarded as program caches.".into() };
        if !left.iter().any(|left| left.location == entry.location && left.kind == entry.kind) {
            left.push(entry);
        }
    }
    let failures = progress.outcomes.values().any(|(outcome, _)| *outcome == Outcome::Failed);
    let local_failure = request
        .steps
        .iter()
        .filter(|step| !matches!(step.action, Action::Logout { .. }))
        .any(|step| progress.outcomes.get(&step.item.id).is_some_and(|(outcome, _)| *outcome != Outcome::Done));
    if let Some(system) = system {
        let deferred = request.installation.application_control.is_some()
            && request.steps.iter().any(|step| matches!(step.action, Action::Package));
        system
            .complete(!local_failure && running_stopped, deferred, &mut |event| {
                record_event(event, &mut progress, &progress_file, &mut events)
            })
            .await?;
    }
    if request.plan.everything
        && !local_failure
        && running_stopped
        && request.installation.application_process.is_none()
    {
        uninstall::delete_file(&progress_file)?;
        uninstall::delete_file(&request.installation.marker())?;
    }
    drop(lock);
    if request.plan.everything {
        for entry in crate::entries(root)? {
            if entry.file_name() == "apply-history" {
                continue;
            }
            left.push(Left {
                kind: "user-files".into(),
                what: "Remaining local directory entry".into(),
                location: Some(entry.path().display().to_string()),
                why: if entry.file_name() == "uninstalled.json" || entry.file_name() == "uninstall-progress.json" {
                    if !local_failure && request.installation.application_process.is_some() {
                        "The startup barrier remains until the application exits and its package takes over.".into()
                    } else {
                        "An incomplete local uninstall retains its checkpoint so it can resume.".into()
                    }
                } else {
                    "The entry was not removed; its earlier step or ownership report identifies why.".into()
                },
            });
        }
    }
    if request.plan.everything && root.try_exists()? && crate::entries(root)?.is_empty() {
        std::fs::remove_dir(root)?;
    }
    events(Event::Finished { left });
    anyhow::ensure!(
        !failures && running_stopped,
        "one or more uninstall steps failed; the final JSON line identifies what remains"
    );
    Ok(())
}

/// The package calls this after the original application exited. Until then
/// its client cannot start a new node in a just-erased World directory.
pub fn finish_application(root: &Path) -> anyhow::Result<()> {
    let Some(request) = crate::read::<Request>(&root.join("uninstalled.json"))? else { return Ok(()) };
    request.installation.validate()?;
    anyhow::ensure!(request.plan.everything, "data-preserving uninstall retains its startup and restoration record");
    let progress: Progress =
        crate::read(&root.join("uninstall-progress.json"))?.context("the uninstall completion record is absent")?;
    anyhow::ensure!(
        request
            .steps
            .iter()
            .filter(|step| !matches!(step.action, Action::Logout { .. }))
            .all(|step| progress.outcomes.get(&step.item.id).is_some_and(|(outcome, _)| *outcome == Outcome::Done)),
        "the local kernel removal has not completed"
    );
    uninstall::delete_file(&root.join("uninstall-progress.json"))?;
    uninstall::delete_file(&root.join("uninstalled.json"))?;
    if crate::entries(root)?.is_empty() {
        std::fs::remove_dir(root)?;
    }
    Ok(())
}

fn remaining(request: &Request, progress: &Progress) -> Vec<Left> {
    let mut left = progress.retained.clone();
    for item in &request.plan.keeps {
        left.push(Left { kind: item.kind.clone(), what: item.what.clone(), location: item.location.clone(),
            why: if item.kind == "user-files" { "These are original user file objects retained across publication. A later write through an old handle must not be discarded.".into() }
                else if item.kind == "other-program" { "This copy is owned by npm or a separate installation.".into() } else { "Data-preserving uninstall was selected.".into() } });
    }
    for step in &request.steps {
        if let Some((outcome, why)) = progress.outcomes.get(&step.item.id)
            && *outcome != Outcome::Done
        {
            left.push(Left {
                kind: step.item.kind.clone(),
                what: step.item.what.clone(),
                location: step.item.location.clone(),
                why: why.clone().unwrap_or_else(|| "This step was not completed.".into()),
            });
        }
        if let Action::Trust { home, .. } | Action::Host(apply::Integration { host: apply::Host::Claude, home, .. }) =
            &step.action
        {
            let entry = Left { kind: "claude-permission".into(), what: "skipDangerousModePermissionPrompt".into(), location: Some(home.join("settings.json").display().to_string()), why: "No evidence of the value before World changed it is recorded; the user's permission is left unchanged.".into() };
            if !left.contains(&entry) {
                left.push(entry);
            }
        }
        if matches!(step.action, Action::Application | Action::Package) {
            left.push(Left {
                kind: "app".into(),
                what: "World application and package-owned registrations".into(),
                location: step.item.location.clone(),
                why: "The application must exit and hand these to its own package uninstaller.".into(),
            });
        }
    }
    #[cfg(windows)]
    left.push(Left {
        kind: "account-right".into(),
        what: "Log on as a batch job".into(),
        location: Some(request.installation.user_sid.clone()),
        why: "No evidence that the account lacked this right before installation is recorded.".into(),
    });
    #[cfg(target_os = "linux")]
    if request.installation.root == request.installation.home.join(".fastctx") {
        left.push(Left {
            kind: "linger".into(),
            what: "Legacy Linux user-manager linger".into(),
            location: Some(request.installation.uid.to_string()),
            why: "The unpublished installer did not record whether linger was already enabled.".into(),
        });
    }
    if let Some(other) = &request.plan.other {
        left.push(Left {
            kind: "program".into(),
            what: other.version.clone().unwrap_or_else(|| "Another fastctx installation".into()),
            location: Some(other.path.clone()),
            why: "This copy is owned by npm or a separate installation.".into(),
        });
    }
    left
}

async fn perform(request: &Request, step: &Step, progress: &mut Progress) -> anyhow::Result<()> {
    let installation = &request.installation;
    let places = installation.places(std::env::current_exe()?);
    match &step.action {
        Action::Host(home) => {
            let selected = places.at_home(home.host, &home.home)?;
            let report = apply::unapply(&selected, home.host.only())?;
            for notice in report.notices {
                progress.retained.push(Left {
                    kind: step.item.kind.clone(),
                    what: "User-modified host configuration".into(),
                    location: Some(notice.location),
                    why: notice.reason,
                });
            }
            anyhow::ensure!(report.leftovers.is_empty(), "{}", report.leftovers.join("; "));
        }
        Action::Legacy(legacy) => legacy.uninstall(installation)?,
        Action::Trust { home, key } => {
            let configured = if home == &installation.claude_home {
                installation.claude_override.as_deref()
            } else if home == &installation.home.join(".claude") {
                None
            } else {
                Some(home.as_path())
            };
            fastctx_hosts::claude::trust::remove_world_trust(configured.map(Path::as_os_str), key)?;
        }
        Action::Path => {
            uninstall::remove_path(&installation.home, &installation.root.join("bin"))?;
        }
        Action::Programs => {
            for path in uninstall::remove_programs(&installation.root)? {
                progress.retained.push(Left {
                    kind: "user-files".into(),
                    what: "Unrecognized addition to a program directory".into(),
                    location: Some(path.display().to_string()),
                    why: "It has no fastctx ownership record.".into(),
                });
            }
        }
        Action::WorldData => {
            let world = installation.root.join("world");
            for entry in crate::entries(&world)? {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if matches!(
                    name.as_ref(),
                    "outside"
                        | "towns"
                        | "removed-residents"
                        | "net"
                        | "net-third-version"
                        | "device.key"
                        | "seal.key"
                        | "machine.json"
                        | "node.lock"
                        | "node.log"
                        | "node.sock"
                        | "node.service"
                        | "write.lock"
                        | "alone.json"
                        | "claude"
                        | "claude-calls"
                        | "claude-plugins"
                        | "codex"
                        | "codex-reports"
                        | "claude-controller.json"
                        | "resident-cache"
                        | "agent-homes.json"
                        | "claude-consent.json"
                        | "colleagues"
                        | "elsewhere"
                        | "on-stops"
                        | "on-deliveries"
                        | "letter-parts"
                ) || name.starts_with("net-left-")
                    || (name.starts_with("job-") && name.ends_with(".sock"))
                {
                    uninstall::delete_tree(&entry.path())?;
                } else {
                    progress.retained.push(Left {
                        kind: "user-files".into(),
                        what: "Unrecognized addition to the World directory".into(),
                        location: Some(entry.path().display().to_string()),
                        why: "It is not an owned World data path.".into(),
                    });
                }
            }
            if world.try_exists()? && crate::entries(&world)?.is_empty() {
                std::fs::remove_dir(world)?;
            }
        }
        Action::Delete(path) => uninstall::delete_tree(path)?,
        Action::Application => {}
        _ => anyhow::bail!("this operation belongs to another uninstall phase"),
    }
    Ok(())
}

fn step_event(step: &Step, result: anyhow::Result<()>) -> Event {
    match result {
        Ok(()) => Event::Step { step: step.item.id.clone(), result: Outcome::Done, why: None },
        Err(error) => {
            Event::Step { step: step.item.id.clone(), result: Outcome::Failed, why: Some(format!("{error:#}")) }
        }
    }
}

fn record(
    step: &Step,
    result: anyhow::Result<()>,
    progress: &mut Progress,
    file: &Path,
    events: &mut impl FnMut(Event),
) -> anyhow::Result<()> {
    let event = step_event(step, result);
    record_event(event, progress, file, events)
}

fn record_event(
    event: Event,
    progress: &mut Progress,
    file: &Path,
    events: &mut impl FnMut(Event),
) -> anyhow::Result<()> {
    if let Event::Finished { left } = event {
        progress.retained.extend(left);
        return publish(file, progress);
    }
    if let Event::Step { step, result, why } = &event {
        progress.outcomes.insert(step.clone(), (*result, why.clone()));
    }
    publish(file, progress)?;
    events(event);
    Ok(())
}

pub fn run_system(request: &Request, mut events: impl FnMut(Event)) {
    run_system_phase(request, true, &mut events);
    run_system_phase(request, false, &mut events);
}

pub(crate) fn run_system_phase(request: &Request, early: bool, mut events: impl FnMut(Event)) {
    let mut removed = BTreeMap::new();
    let is_early = |step: &Step| {
        match &step.action {
        Action::Service(registration) => registration.kind == fastctx_install::service::Kind::Node,
        Action::ServicePart(name) => request.steps.iter().any(|step| matches!(&step.action, Action::Service(registration) if registration.name == *name && registration.kind == fastctx_install::service::Kind::Node)),
        Action::Legacy(_) => true,
        _ => false,
    }
    };
    for step in request.steps.iter().filter(|step| step.item.admin && is_early(step) == early) {
        let result = (|| {
            anyhow::ensure!(
                fastctx_platform::elevation::is_elevated()?,
                "the system phase requires administrator rights"
            );
            match &step.action {
                Action::Service(registration) => {
                    let result = registration.uninstall(&request.installation);
                    removed.insert(registration.name.clone(), result.is_ok());
                    result.map(|paths| {
                        if !paths.is_empty() {
                            events(Event::Finished {
                                left: paths
                                    .into_iter()
                                    .map(|path| Left {
                                        kind: "user-files".into(),
                                        what: "Unrecognized addition to a service directory".into(),
                                        location: Some(path.display().to_string()),
                                        why: "It has no fastctx ownership record.".into(),
                                    })
                                    .collect(),
                            });
                        }
                    })
                }
                Action::ServicePart(name) => {
                    anyhow::ensure!(removed.get(name) == Some(&true), "the service cleanup did not complete");
                    Ok(())
                }
                Action::Legacy(legacy) => legacy.uninstall(&request.installation),
                Action::Package => Ok(()),
                Action::HubData(directory) => {
                    if !directory.try_exists()? {
                        return Ok(());
                    }
                    for entry in crate::entries(directory)? {
                        let name = entry.file_name();
                        let name = name.to_string_lossy();
                        if matches!(
                            name.as_ref(),
                            "hub.json"
                                | "directory.json"
                                | "members.json"
                                | "login.secret"
                                | "letters.json"
                                | "worked.json"
                                | "hub.log"
                                | "directory.before-sharing.json"
                                | "directory.upgraded.txt"
                                | "ledger"
                                | "outbox"
                        ) {
                            uninstall::delete_tree(&entry.path())?;
                        } else if name == "tls" {
                            for name in ["certificate.der", "key.der"] {
                                uninstall::delete_file(&entry.path().join(name))?;
                            }
                            if crate::entries(&entry.path())?.is_empty() {
                                std::fs::remove_dir(entry.path())?;
                            }
                        }
                    }
                    anyhow::ensure!(
                        crate::entries(directory)?.is_empty(),
                        "the hub directory also contains user-owned or externally maintained files; those were preserved"
                    );
                    std::fs::remove_dir(directory)?;
                    Ok(())
                }
                _ => anyhow::bail!("this operation does not belong to the system phase"),
            }
        })();
        events(step_event(step, result));
    }
}
