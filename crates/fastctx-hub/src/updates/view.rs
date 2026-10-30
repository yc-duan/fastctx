//! Update views and arrangements use the same account authority as node admission.

use crate::Hub;
use fastctx_net::directory::{Directory, Member};
use fastctx_net::wire::{Change, Told};
use fastctx_types::update::{
    ASKED_SINCE, ActionReason, Actions, Channel, FailureReason, Kind, MachineUpdate, Permission, Problem, Scheduled,
    Stage, View, ViewCall, ViewReply, release,
};
use fastctx_types::{AccountName, MachineName};
use std::sync::Arc;

pub(super) fn authenticate(directory: &Directory, asker: &MachineName, account: &AccountName) -> Result<(), String> {
    if !directory.logged_in(asker, account) || directory.account(account).is_none_or(|a| a.locked) {
        return Err(format!("{asker} is not logged in to an unlocked account {account}"));
    }
    Ok(())
}

fn actions(member: &Member, kernel: Option<&str>, online: bool, phone: bool, app: bool, modern: bool) -> Actions {
    let base = if phone {
        Permission::no(ActionReason::Phone)
    } else if kernel.is_none() {
        Permission::no(ActionReason::UnknownVersion)
    } else if kernel.and_then(release).is_none() {
        Permission::no(ActionReason::Development)
    } else {
        Permission::yes()
    };
    let choose = if base.allowed && kernel.and_then(release) < release(ASKED_SINCE) {
        Permission::no(ActionReason::TooOld)
    } else {
        base.clone()
    };
    let settings = if !online { Permission::no(ActionReason::Offline) } else { choose.clone() };
    // Old application nodes accepted a queued update but never executed it.
    // They still accept a direct update while online.
    let schedule = if choose.allowed && app && !modern { Permission::no(ActionReason::TooOld) } else { choose.clone() };
    let repair = base.clone();
    let update = if !online && !schedule.allowed { Permission::no(ActionReason::Offline) } else { base };
    let _ = member;
    Actions {
        repair: Some(repair),
        update,
        choose,
        settings,
        schedule,
        cancel: if phone { Permission::no(ActionReason::Phone) } else { Permission::yes() },
    }
}

pub(super) fn snapshot(hub: &Hub, asker: &MachineName, account: &AccountName) -> Result<View, String> {
    let store = hub.store();
    let directory = &store.book.directory;
    authenticate(directory, asker, account)?;
    let index = hub.index_of(&store);
    let indexed = fastctx_net::directory_index::Indexed::new(directory, &index);
    let positions = fastctx_net::directory_index::ordered(
        super::announce::managed(&indexed, account)
            .into_iter()
            .chain(indexed.logged(account).iter().copied().filter(|&at| directory.members[at].home.is_none())),
    );
    let mut due = std::collections::BTreeMap::new();
    let mut repairs = std::collections::BTreeMap::new();
    for kept in &store.hub.due {
        due.entry(kept.key).or_insert(&kept.due);
    }
    for kept in &store.hub.repairs {
        repairs.entry(kept.key).or_insert(&kept.due);
    }
    let sessions = hub.sessions();
    let updates = hub.updates.lock().expect("never poisoned");
    let network = updates.network(store.hub.updates.clone(), Default::default());
    let mut releases: Vec<_> = updates.manifests().collect();
    releases.sort_by_key(|m| std::cmp::Reverse(m.semver()));
    releases.dedup_by(|a, b| a.version == b.version);
    let mut versions = std::collections::BTreeMap::new();
    for listed in &updates.looked.releases {
        versions.insert(
            listed.version.clone(),
            fastctx_types::update::VersionChoice {
                version: listed.version.clone(),
                published: listed.published.map(|at| at.to_string()),
                summary: None,
                manifest: None,
                problem: None,
            },
        );
    }
    for manifest in &releases {
        versions.insert(
            manifest.version.clone(),
            fastctx_types::update::VersionChoice {
                version: manifest.version.clone(),
                published: manifest.pub_date.clone(),
                summary: manifest.notes.as_ref().and_then(|notes| notes.lines().next()).map(str::to_owned),
                manifest: Some(manifest.clone()),
                problem: None,
            },
        );
    }
    let mut versions: Vec<_> = versions.into_values().collect();
    versions.sort_by_key(|choice| std::cmp::Reverse(release(&choice.version)));
    let mut machines = Vec::new();
    for at in positions {
        let member = &directory.members[at];
        let phone = member.home.is_none();
        let last = updates.last.get(&member.key);
        let report = last.map(|l| &l.report);
        let kernel = report.map(|r| r.kernel.clone());
        let app = report.and_then(|r| r.app.clone());
        let running = app.as_ref().map(|a| a.version.as_str()).or(kernel.as_deref());
        let channel = Channel::of(running.unwrap_or_default());
        let target = app.as_ref().map(|a| a.target.clone()).or_else(|| report.and_then(|r| r.target.clone()));
        let offer = running
            .zip(target.as_deref())
            .and_then(|(running, target)| fastctx_update::state::latest_for(releases.iter(), running, target, None))
            .cloned();
        let kind = if phone {
            Kind::Phone
        } else if member.managed {
            Kind::Managed
        } else if app.is_some() {
            Kind::App
        } else {
            Kind::Headless
        };
        let online = sessions.contains_key(&member.name);
        let actions =
            actions(member, kernel.as_deref(), online, phone, app.is_some(), report.is_some_and(|r| r.updates_view));
        machines.push(MachineUpdate {
            machine: member.name.clone(),
            owner: member.home.clone().unwrap_or_else(|| account.clone()),
            group: member.group.clone(),
            kind,
            online,
            reported: last.map(|l| l.at),
            kernel,
            app,
            target,
            channel,
            auto: report.and_then(|r| r.auto),
            auto_chosen: !member.managed && report.is_some_and(|r| r.auto_chosen),
            relays: report.map(|r| r.relays),
            github: report.and_then(|r| r.github),
            github_at: report.and_then(|r| r.github_at),
            check_failure: report.and_then(|r| r.check_failure.clone()),
            checking: online && report.is_some_and(|r| r.checking),
            stage: report.filter(|r| r.updates_view).map_or(Stage::Unknown, |r| r.stage.clone()),
            due: due.get(&member.key).map(|due| fastctx_types::update::Due {
                version: due.version.clone(),
                by: due.by.clone(),
                at: due.at,
                manifest: None,
                problem: due.problem.clone(),
            }),
            offer,
            actions,
            repair_supported: report.is_some_and(|r| r.repair_supported),
            repair_due: repairs.get(&member.key).map(|due| fastctx_types::update::RepairDue {
                id: due.id.clone(),
                version: due.version.clone(),
                by: due.by.clone(),
                at: due.at,
                manifest: None,
                problem: due.problem.clone(),
                attempts: due.attempts,
                retry_at: due.retry_at,
                observed: due.observed,
            }),
            repair: report.and_then(|r| r.repair.clone()),
        });
    }
    let available = machines.iter().filter(|m| m.offer.is_some() && m.actions.update.allowed).count();
    let beta = network.on(Channel::Beta).cloned();
    Ok(View {
        hub: env!("CARGO_PKG_VERSION").into(),
        stable: network.stable,
        beta,
        releases,
        versions,
        policy: directory.is_super(account).then(|| store.hub.updates.clone()),
        machines,
        available,
        repair_supported: true,
    })
}

pub(crate) async fn request(
    hub: &Arc<Hub>,
    asker: &MachineName,
    account: &AccountName,
    request: ViewCall,
) -> Result<Told, String> {
    if matches!(request, ViewCall::Repair { .. } | ViewCall::CancelRepair { .. } | ViewCall::RepairResult { .. }) {
        return super::repair::request(hub, asker, account, request).await;
    }
    let mut view = snapshot(hub, asker, account)?;
    let needed = match &request {
        ViewCall::Release { version } | ViewCall::Schedule { version: Some(version), .. } => Some(version),
        _ => None,
    };
    let mut manifest_problem = None;
    if let Some(version) = needed
        && !view.releases.iter().any(|manifest| &manifest.version == version)
    {
        manifest_problem = read_manifest(hub, version).await.err();
        // Authority can change during IO; the refreshed snapshot and each
        // committed arrangement both check it again.
        view = snapshot(hub, asker, account)?;
    }
    let reply = match request {
        ViewCall::Repair { .. } | ViewCall::CancelRepair { .. } | ViewCall::RepairResult { .. } => {
            unreachable!("repair was dispatched above")
        }
        ViewCall::Release { version } => {
            let mut choice = view.versions.into_iter().find(|choice| choice.version == version).unwrap_or(
                fastctx_types::update::VersionChoice {
                    version,
                    published: None,
                    summary: None,
                    manifest: None,
                    problem: None,
                },
            );
            if choice.manifest.is_none() {
                choice.problem = manifest_problem.map(|why| Problem::new(FailureReason::Other, why));
            }
            return Ok(Told::NetworkUpdates { reply: ViewReply::Release { choice: Box::new(choice) } });
        }
        ViewCall::View => return Ok(Told::NetworkUpdates { reply: ViewReply::View { view: Box::new(view) } }),
        ViewCall::Policy { minimum, managed_auto } => {
            let mut policy = hub.store().hub.updates.clone();
            policy.minimum = minimum;
            policy.managed_auto = managed_auto;
            let (result, ended) = crate::manage::change(hub, asker, account, Change::Updates { policy });
            result?;
            crate::manage::end(hub, ended);
            ViewReply::Done
        }
        ViewCall::Schedule { machines, version } => {
            let mut rows = std::collections::HashMap::new();
            for row in &view.machines {
                rows.entry(&row.machine).or_insert(row);
            }
            let mut results = Vec::with_capacity(machines.len());
            let mut pending = Vec::new();
            for machine in machines {
                let row = rows.get(&machine).copied();
                let refusal = match row {
                    None => {
                        let store = hub.store();
                        Some(match store.book.directory.member(&machine) {
                            None => ActionReason::MissingMachine,
                            Some(m) if m.home.as_ref().is_some_and(|o| store.book.directory.is_visitor(o)) => {
                                ActionReason::Visitor
                            }
                            Some(_) => ActionReason::NotOwner,
                        })
                    }
                    Some(row) => {
                        let permission = if version.is_some() { &row.actions.schedule } else { &row.actions.cancel };
                        if !permission.allowed {
                            permission.reason.clone()
                        } else if let Some(version) = &version {
                            match view.releases.iter().find(|m| m.version == *version) {
                                None => Some(ActionReason::Unavailable),
                                Some(m)
                                    if !m.newer_than(
                                        row.app
                                            .as_ref()
                                            .map(|a| a.version.as_str())
                                            .or(row.kernel.as_deref())
                                            .unwrap_or_default(),
                                    ) =>
                                {
                                    Some(ActionReason::NotNewer)
                                }
                                Some(m) if row.target.as_deref().is_some_and(|t| m.package(t).is_none()) => {
                                    Some(ActionReason::NoPackage)
                                }
                                Some(_) => None,
                            }
                        } else {
                            None
                        }
                    }
                };
                if let Some(reason) = refusal {
                    results.push(Scheduled {
                        machine,
                        done: false,
                        why: Some(if reason == ActionReason::Unavailable {
                            manifest_problem.clone().unwrap_or_else(|| "this version's manifest is not known".into())
                        } else {
                            format!("cannot arrange this update: {reason:?}")
                        }),
                        reason: Some(reason),
                    });
                    continue;
                }
                pending.push(results.len());
                results.push(Scheduled { machine, done: false, reason: None, why: None });
            }
            // Bound each durable transaction, not the number the user can arrange.
            for chunk in pending.chunks(64) {
                let machines: Vec<_> = chunk.iter().map(|&at| results[at].machine.clone()).collect();
                let committed = super::due_batch::apply(hub, asker, account, &machines, version.as_deref());
                for (&at, result) in chunk.iter().zip(committed) {
                    match result {
                        Ok(()) => results[at].done = true,
                        Err(why) => {
                            results[at].reason = Some(ActionReason::Other);
                            results[at].why = Some(why);
                        }
                    }
                }
                tokio::task::yield_now().await;
            }
            ViewReply::Scheduled { results }
        }
    };
    super::prefetch(hub);
    super::tell(hub, None);
    super::prune(hub);
    Ok(Told::NetworkUpdates { reply })
}

pub(super) fn problem(hub: &Hub, version: &str, target: Option<&str>, why: Option<String>) {
    let cache = hub.updates.lock().expect("never poisoned").cache.clone();
    let available = super::inventory::held(hub, &cache);
    let mut store = hub.store();
    // Background preparation may outlive the final moving snapshot
    // (2026-10-29); its late outcome is retried by the receiving hub.
    if hub.moving.frozen() {
        return;
    }
    let targets = store.hub.package_targets.clone();
    let problem = why.map(|why| Problem { reason: FailureReason::Other, why });
    let mut changed = false;
    for due in &mut store.hub.due {
        if due.due.version != version {
            continue;
        }
        let needs = targets.get(&due.key).cloned().unwrap_or_default();
        let packages = due.due.manifest.as_ref().map(|manifest| super::inventory::for_targets(manifest, &needs));
        if target.is_some_and(|target| {
            packages.as_ref().is_some_and(|packages| !packages.iter().any(|p| p.target == target))
        }) {
            continue;
        }
        // Success of another platform never erases this machine's failed preparation.
        if problem.is_none()
            && !packages.as_ref().is_some_and(|packages| packages.iter().all(|p| available.contains(p)))
        {
            continue;
        }
        if due.due.problem != problem {
            std::sync::Arc::make_mut(due).due.problem = problem.clone();
            changed = true;
        }
    }
    for repair in &mut store.hub.repairs {
        if repair.due.version != version {
            continue;
        }
        let needs = targets.get(&repair.key).cloned().unwrap_or_default();
        let packages = repair.due.manifest.as_ref().map(|m| super::inventory::for_targets(m, &needs));
        if target.is_some_and(|target| {
            packages.as_ref().is_some_and(|packages| !packages.iter().any(|p| p.target == target))
        }) {
            continue;
        }
        if problem.is_none()
            && !packages.as_ref().is_some_and(|packages| packages.iter().all(|p| available.contains(p)))
        {
            continue;
        }
        if repair.due.observed.is_none() && repair.due.problem != problem {
            std::sync::Arc::make_mut(repair).due.problem = problem.clone();
            changed = true;
        }
    }
    if changed && let Err(error) = store.save_hub() {
        hub.log.line(format_args!("keeping update preparation outcome: {error}"));
    }
}

/// Retrieves only the version explicitly selected, outside the hub's shared locks.
pub(super) async fn read_manifest(hub: &Hub, version: &str) -> Result<(), String> {
    if release(version).is_none() {
        return Err(format!("{version} is not a release version"));
    }
    let (known, releases, cache) = {
        let updates = hub.updates.lock().expect("never poisoned");
        (updates.manifest(version).is_some(), updates.looked.releases.clone(), updates.cache.clone())
    };
    if known {
        return Ok(());
    }
    let manifest = tokio::time::timeout(
        std::time::Duration::from_secs(50),
        fastctx_update::github::manifest_of(&releases, version),
    )
    .await
    .map_err(|_| format!("reading the manifest of {version} took longer than 50 seconds"))??;
    let saving = manifest.clone();
    tokio::task::spawn_blocking(move || cache.keep_manifest(&saving)).await.map_err(|error| error.to_string())??;
    hub.updates.lock().expect("never poisoned").catalog.insert(manifest.version.clone(), manifest);
    Ok(())
}
