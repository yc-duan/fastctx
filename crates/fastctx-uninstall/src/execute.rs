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

pub async fn run(mut request: Request, mut events: impl FnMut(Event)) -> anyhow::Result<()> {
    request.installation.validate()?;
    let root = &request.installation.root;
    let world = fastctx_types::layout::WorldDir::under(root);
    // A managed machine its owner has not taken off keeps Fastctx, unless
    // an administrator of this computer takes it off.
    if let Some(managed) = crate::managed(&world)?
        && !fastctx_platform::elevation::is_administrator()?
    {
        anyhow::bail!("{}", managed.refusal());
    }
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
    let progress_file = root.join("uninstall-progress.json");
    // A complete removal that replaces one keeping the data starts its own
    // checkpoint: the earlier one names the steps of another plan.
    if crate::read::<Request>(&request.installation.marker())?
        .is_some_and(|previous| previous.plan.everything != request.plan.everything)
    {
        uninstall::delete_file(&progress_file)?;
    }
    publish(&request.installation.marker(), &request)?;
    let mut progress = crate::read::<Progress>(&progress_file)?.unwrap_or_default();
    // Copies of the hosts' settings, which can carry a login's token, do not
    // outlive a removal of either kind (`world-run.md`, apply).
    progress.retained.retain(|left| left.kind != "settings-copies");
    for problem in apply::settle(root) {
        progress.retained.push(Left {
            kind: "settings-copies".into(),
            what: "Copies of host settings kept while an apply ran".into(),
            location: Some(root.display().to_string()),
            why: problem,
        });
    }
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
                    if !done(step, &progress) {
                        record(step, Err(anyhow::anyhow!("{error:#}")), &mut progress, &progress_file, &mut events)?;
                    }
                }
                None
            }
        }
    };
    let mut node = None;
    let mut running_stopped = true;
    for step in request.steps.iter().filter(|step| !step.item.admin && matches!(step.action, Action::Legacy(_))) {
        let Action::Legacy(legacy) = &step.action else { unreachable!() };
        record(step, legacy.uninstall(&request.installation), &mut progress, &progress_file, &mut events)?;
    }
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
    let system_removed = request.steps.iter().filter(|step| boot_registration(step)).all(|step| done(step, &progress));
    if running_stopped
        && system_removed
        && let Err(error) = crate::runtime::finish(node.as_ref()).await
    {
        running_stopped = false;
        if let Some(step) = request.steps.iter().find(|step| matches!(step.action, Action::Stop)) {
            record(step, Err(anyhow::anyhow!("{error:#}")), &mut progress, &progress_file, &mut events)?;
        }
        progress.retained.push(Left {
            kind: "runtime".into(),
            what: "World node".into(),
            location: node.as_ref().map(|identity| identity.pid.to_string()),
            why: format!("{error:#}"),
        });
    }
    if running_stopped && system_removed {
        // Persist identities found after the preview before their diagnostic
        // logs can disappear. A failed temp deletion still has its evidence.
        crate::claude_work_cache::remember_sessions(&mut request.steps)?;
        publish(&request.installation.marker(), &request)?;
    }
    for step in request.steps.iter().filter(|step| {
        !step.item.admin && !matches!(step.action, Action::Stop | Action::Logout { .. } | Action::Legacy(_))
    }) {
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
            Err(anyhow::anyhow!("{}", blocked_by(&request, &progress)))
        } else if matches!(
            step.action,
            Action::Programs
                | Action::Delete(_)
                | Action::WorldData
                | Action::ApplicationData(_)
                | Action::ClaudeWorkCache { .. }
                | Action::ClaudeSessionCache { .. }
        ) && !hosts_withdrawn
            && !matches!(&step.action, Action::Delete(path) if path == &world.node_service())
        {
            Err(anyhow::anyhow!(
                "a host integration could not be withdrawn; its restoration records and program assets must remain for retry"
            ))
        } else {
            perform(&request, step, &mut progress).await
        };
        record(step, result, &mut progress, &progress_file, &mut events)?;
    }
    if running_stopped {
        progress.retained.retain(|left| left.kind != "runtime" || left.what != "World node");
    }
    let mut retained = Vec::new();
    for left in std::mem::take(&mut progress.retained) {
        if matches!(left.kind.as_str(), "user-files" | "unrecognized" | "claude-shared-cache")
            && let Some(path) = left.location.as_deref().map(Path::new).filter(|path| path.is_absolute())
        {
            match std::fs::symlink_metadata(path) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
                Ok(_) => {}
            }
        }
        retained.push(left);
    }
    progress.retained = retained;
    progress.retained.sort_by(|left, right| {
        (&left.kind, &left.location, &left.why).cmp(&(&right.kind, &right.location, &right.why))
    });
    progress.retained.dedup();
    let mut left = remaining(&request, &progress, &places);
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
    let failures = request
        .steps
        .iter()
        .any(|step| progress.outcomes.get(&step.item.id).is_some_and(|(outcome, _)| *outcome == Outcome::Failed));
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
            if entry.file_name() == "apply-history"
                || request
                    .plan
                    .keeps
                    .iter()
                    .any(|item| item.location.as_deref() == Some(&entry.path().display().to_string()))
            {
                continue;
            }
            // The removal's own checkpoint and barrier are its records, not
            // the user's files.
            let records = entry.file_name() == "uninstalled.json" || entry.file_name() == "uninstall-progress.json";
            left.push(Left {
                kind: if records { "removal-records" } else { "unremoved" }.into(),
                what: "Remaining local directory entry".into(),
                location: Some(entry.path().display().to_string()),
                why: if records {
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

fn done(step: &Step, progress: &Progress) -> bool {
    progress.outcomes.get(&step.item.id).is_some_and(|(outcome, _)| *outcome == Outcome::Done)
}

fn boot_registration(step: &Step) -> bool {
    matches!(step.action, Action::Service(_) | Action::ServicePart(_) | Action::Legacy(_))
}

fn blocked_by(request: &Request, progress: &Progress) -> String {
    blocked_steps(
        request.steps.iter().filter(|step| matches!(step.action, Action::Stop) || boot_registration(step)),
        progress,
    )
}

fn blocked_steps<'a>(steps: impl Iterator<Item = &'a Step>, progress: &Progress) -> String {
    let reasons = steps
        .filter(|step| !done(step, progress))
        .map(|step| {
            let reason = progress
                .outcomes
                .get(&step.item.id)
                .and_then(|(_, why)| why.as_deref())
                .unwrap_or("No completion was recorded.");
            format!(
                "{}: {} ({}): {reason}",
                step.item.id,
                step.item.what,
                step.item.location.as_deref().unwrap_or("location unavailable")
            )
        })
        .collect::<Vec<_>>();
    format!(
        "Dependent cleanup was not performed because these uninstall steps did not complete: {}",
        reasons.join("; ")
    )
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
    let left = application_data(&request.installation.application_dirs)?;
    if !left.is_empty() {
        println!("{}", serde_json::to_string(&Event::Finished { left })?);
    }
    uninstall::delete_file(&root.join("uninstall-progress.json"))?;
    uninstall::delete_file(&root.join("uninstalled.json"))?;
    if crate::entries(root)?.is_empty() {
        std::fs::remove_dir(root)?;
    }
    Ok(())
}

pub(crate) fn application_data(directories: &[std::path::PathBuf]) -> anyhow::Result<Vec<Left>> {
    let mut left = Vec::new();
    let mut failed = Vec::new();
    for directory in directories {
        let data = uninstall::remove_application_data(directory);
        left.extend(data.left.into_iter().map(|path| Left {
            kind: "user-files".into(),
            what: "Unrecognized addition to the application data directory".into(),
            location: Some(path.display().to_string()),
            why: "It is not an owned application data path.".into(),
        }));
        failed.extend(data.failed.into_iter().map(|(path, error)| format!("{}: {error}", path.display())));
    }
    anyhow::ensure!(failed.is_empty(), "{}", failed.join("; "));
    Ok(left)
}

fn remaining(request: &Request, progress: &Progress, places: &apply::Places) -> Vec<Left> {
    let mut left = progress.retained.clone();
    for item in &request.plan.keeps {
        if item.kind == "claude-shared-cache"
            && let Some(location) = &item.location
            && request.steps.iter().any(|step| {
                (matches!(&step.action, Action::ClaudeWorkCache { directory, .. } if Path::new(location).starts_with(directory))
                    || matches!(&step.action, Action::ClaudeSessionCache { path, .. } if Path::new(location) == path))
                    && done(step, progress)
            })
        {
            continue;
        }
        left.push(Left { kind: item.kind.clone(), what: item.what.clone(), location: item.location.clone(),
            why: if item.what == "Old-version files with unproven or user-modified contents" { "The old version did not prove these contents were unchanged; user additions and edits must be preserved.".into() }
                else if item.what == "Claude cache entry without exclusive World ownership" { "Claude's cache paths can be shared or collide; this entry could not be identified as belonging only to World.".into() }
                else if item.kind == "user-files" { "These are original user file objects retained across publication. A later write through an old handle must not be discarded.".into() }
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
            // Named only where the settings give it: a login World never
            // asked has nothing of it to report.
            let why = match apply::claude_consent_at(places, home) {
                Ok(false) => None,
                Ok(true) => Some("No evidence of the value before World changed it is recorded; the user's permission is left unchanged.".to_owned()),
                Err(error) => Some(format!("Whether it is set could not be read: {error}")),
            };
            if let Some(why) = why {
                let entry = Left {
                    kind: "claude-permission".into(),
                    what: "skipDangerousModePermissionPrompt".into(),
                    location: Some(home.join("settings.json").display().to_string()),
                    why,
                };
                if !left.contains(&entry) {
                    left.push(entry);
                }
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

/// Claude Code conversations left open hold the World's mod, which reads the
/// removal's marker within one look and from then on writes nothing and starts
/// no helper. Programs and data are taken once two looks have passed since the
/// marker was written, so none of them puts a file back or holds a program.
async fn mods_stood_aside(installation: &uninstall::Installation) -> anyhow::Result<()> {
    let marked = std::fs::metadata(installation.marker())?.modified()?;
    let ready = marked + 2 * fastctx_hosts::claude::MOD_LOOK;
    if let Ok(wait) = ready.duration_since(std::time::SystemTime::now()) {
        tokio::time::sleep(wait).await;
    }
    Ok(())
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
        Action::ClaudeCache(home) => {
            let withdrawal = request
                .steps
                .iter()
                .find(|step| {
                    matches!(&step.action, Action::Host(integration)
                    if integration.host == apply::Host::Claude && &integration.home == home)
                })
                .context("the Claude cache has no integration withdrawal step")?;
            anyhow::ensure!(done(withdrawal, progress), "{}", blocked_steps(std::iter::once(withdrawal), progress));
            mods_stood_aside(installation).await?;
            uninstall::delete_tree(&crate::claude_cache::directory(home))?;
        }
        Action::Legacy(legacy) => legacy.uninstall(installation)?,
        Action::ClaudeWorkCache { .. } | Action::ClaudeSessionCache { .. } => {
            anyhow::ensure!(request.plan.everything, "data-preserving uninstall keeps Claude working-directory caches");
            mods_stood_aside(installation).await?;
            let left = if matches!(step.action, Action::ClaudeWorkCache { .. }) {
                crate::claude_work_cache::delete(&step.action)?
            } else {
                crate::claude_session_cache::delete(&step.action)?
            };
            for path in left {
                progress.retained.push(Left {
                    kind: "claude-shared-cache".into(),
                    what: "Claude cache entry without exclusive World ownership".into(),
                    location: Some(path.display().to_string()),
                    why: "Claude's cache paths can be shared or collide; this entry could not be identified as belonging only to World.".into(),
                });
            }
        }
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
            mods_stood_aside(installation).await?;
            let programs = uninstall::remove_programs(&installation.root);
            for path in programs.left {
                progress.retained.push(Left {
                    kind: "unrecognized".into(),
                    what: "Unrecognized addition to a program directory".into(),
                    location: Some(path.display().to_string()),
                    why: "It has no fastctx ownership record.".into(),
                });
            }
            anyhow::ensure!(
                programs.failed.is_empty(),
                "{}",
                programs
                    .failed
                    .iter()
                    .map(|(path, error)| format!("{}: {error}", path.display()))
                    .collect::<Vec<_>>()
                    .join("; ")
            );
        }
        Action::WorldData => {
            mods_stood_aside(installation).await?;
            let world = installation.root.join("world");
            // The World directory is Fastctx's own, all of it
            // (`world-town.md`, 世界的东西存在哪): it goes whole, never by a
            // list of names that falls behind what later versions keep there.
            // Every entry is tried; one that cannot go does not keep the
            // others, the identity among them, on this computer.
            let mut failed = Vec::new();
            for entry in crate::entries(&world)? {
                if entry.file_name() == "bin" {
                    // The program step owns it and reports what stayed there.
                    continue;
                }
                if let Err(error) = uninstall::delete_tree(&entry.path()) {
                    failed.push(format!("{}: {error}", entry.path().display()));
                }
            }
            anyhow::ensure!(failed.is_empty(), "{}", failed.join("; "));
            if world.try_exists()? && crate::entries(&world)?.is_empty() {
                std::fs::remove_dir(world)?;
            }
        }
        Action::Delete(path) if crate::inventory::is_legacy_archive(path, &installation.root) => {
            progress.retained.push(Left { kind: "user-files".into(),
                what: "Old-version files with unproven or user-modified contents".into(),
                location: Some(path.display().to_string()),
                why: "The old version did not prove these contents were unchanged; user additions and edits must be preserved.".into() });
        }
        Action::Delete(path) => uninstall::delete_tree(path)?,
        Action::ApplicationData(path) => {
            let data = uninstall::remove_application_data(path);
            for path in data.left {
                progress.retained.push(Left {
                    kind: "unrecognized".into(),
                    what: "Unrecognized addition to the application data directory".into(),
                    location: Some(path.display().to_string()),
                    why: "It is not an owned application data path.".into(),
                });
            }
            anyhow::ensure!(
                data.failed.is_empty(),
                "{}",
                data.failed
                    .iter()
                    .map(|(path, error)| format!("{}: {error}", path.display()))
                    .collect::<Vec<_>>()
                    .join("; ")
            );
        }
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

/// Removes the firewall rules named `names` that are there.
fn remove_rules(names: &[String]) -> anyhow::Result<()> {
    #[cfg(windows)]
    for name in names {
        if let Err(code) = fastctx_install::direct_firewall::remove(name)? {
            anyhow::bail!("Windows Firewall did not remove {name} (netsh exit {code:?})");
        }
    }
    #[cfg(not(windows))]
    anyhow::ensure!(names.is_empty(), "firewall rules are removed only on Windows");
    Ok(())
}

/// Takes away the account a managed computer's World lived in, saying what
/// of it stays.
fn remove_account(sid: &str, events: &mut impl FnMut(Event)) -> anyhow::Result<()> {
    #[cfg(windows)]
    {
        let left = fastctx_install::service::managed::remove(sid)?;
        if !left.is_empty() {
            events(Event::Finished {
                left: left
                    .into_iter()
                    .map(|what| Left {
                        kind: "account".into(),
                        what,
                        location: Some(fastctx_install::service::managed::ACCOUNT.into()),
                        why: "Windows still held it while the account was removed.".into(),
                    })
                    .collect(),
            });
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = events;
        anyhow::bail!("the account {sid} is removed only on Windows")
    }
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
                                        kind: "unrecognized".into(),
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
                Action::DirectRules(names) => remove_rules(names),
                Action::Package => Ok(()),
                Action::Account(sid) => remove_account(sid, &mut events),
                Action::HubData(directory) => {
                    if !directory.try_exists()? {
                        return Ok(());
                    }
                    let mut failed = Vec::new();
                    let mut left = Vec::new();
                    for entry in crate::entries(directory)? {
                        let name = entry.file_name();
                        let name = name.to_string_lossy();
                        if matches!(
                            name.as_ref(),
                            "hub.json"
                                | "hub.sqlite3"
                                | "hub.sqlite3-wal"
                                | "hub.sqlite3-shm"
                                | "hub.sqlite3-journal"
                                | ".hub-handover.sqlite3"
                                | ".hub-handover.sqlite3-wal"
                                | ".hub-handover.sqlite3-shm"
                                | ".hub-handover.sqlite3-journal"
                                | "hub.before-sqlite.json"
                                | "directory.before-sqlite.json"
                                | "letters.before-sqlite.json"
                                | "worked.before-sqlite.json"
                                | "updates"
                                | "directory.json"
                                | "members.json"
                                | "login.secret"
                                | "letters.json"
                                | "worked.json"
                                | "hub.log"
                                // What a retired hub keeps: why, and where it
                                // was moving to.
                                | "retired.json"
                                | "moving-out.json"
                                | "directory.before-sharing.json"
                                | "directory.upgraded.txt"
                                | "ledger"
                                | "outbox"
                                // The Linux programs this hub hands out to the
                                // one-line server install.
                                | "releases"
                        ) {
                            if let Err(error) = uninstall::delete_tree(&entry.path()) {
                                failed.push(format!("{}: {error}", entry.path().display()));
                            }
                        } else if name == "tls" {
                            for name in ["certificate.der", "key.der"] {
                                let path = entry.path().join(name);
                                if let Err(error) = uninstall::delete_file(&path) {
                                    failed.push(format!("{}: {error}", path.display()));
                                }
                            }
                            if crate::entries(&entry.path())?.is_empty() {
                                if let Err(error) = std::fs::remove_dir(entry.path()) {
                                    failed.push(format!("{}: {error}", entry.path().display()));
                                }
                            } else {
                                left.extend(
                                    crate::entries(&entry.path())?
                                        .into_iter()
                                        .filter(|entry| {
                                            entry.file_name() != "certificate.der" && entry.file_name() != "key.der"
                                        })
                                        .map(|entry| entry.path()),
                                );
                            }
                        } else {
                            left.push(entry.path());
                        }
                    }
                    if !left.is_empty() {
                        events(Event::Finished {
                            left: left
                                .into_iter()
                                .map(|path| Left {
                                    kind: "user-files".into(),
                                    what: "Unrecognized addition to the hub directory".into(),
                                    location: Some(path.display().to_string()),
                                    why: "It is not an owned hub data path.".into(),
                                })
                                .collect(),
                        });
                    }
                    anyhow::ensure!(failed.is_empty(), "{}", failed.join("; "));
                    if crate::entries(directory)?.is_empty() {
                        std::fs::remove_dir(directory)?;
                    }
                    Ok(())
                }
                _ => anyhow::bail!("this operation does not belong to the system phase"),
            }
        })();
        events(step_event(step, result));
    }
}
