//! Repair arrangements have their own identity and completion receipt. Version
//! announcements alone can never settle one, including a same-version repair.

use crate::Hub;
use fastctx_net::wire::Told;
use fastctx_types::update::{
    ActionReason, Channel, FailureReason, Problem, REPAIR_SINCE, Scheduled, Stage, ViewCall, ViewReply, release,
};
use fastctx_types::{AccountName, MachineName};
use std::sync::Arc;

pub(super) async fn request(
    hub: &Arc<Hub>,
    asker: &MachineName,
    account: &AccountName,
    call: ViewCall,
) -> Result<Told, String> {
    let view = super::view::snapshot(hub, asker, account)?;
    if let ViewCall::RepairResult { machine, result } = call {
        {
            let store = hub.store();
            let directory = &store.book.directory;
            super::view::authenticate(directory, asker, account)?;
            let target = directory.member(&machine).ok_or("repair target no longer exists")?;
            if !directory.may_update(account, &machine) {
                return Err(fastctx_types::sharing::MANAGER_REQUIRED.into());
            }
            let helper = directory.member(asker).ok_or("repair helper no longer exists")?;
            let updates = hub.updates.lock().expect("repair helper identity");
            if !updates.last.get(&helper.key).is_some_and(|r| r.report.hub_here) {
                return Err("only the node on this hub's machine reports legacy repairs".into());
            }
            if !store.hub.repairs.iter().any(|r| r.key == target.key && r.due.id == result.id) {
                return Err("this repair arrangement was cancelled or replaced".into());
            }
        }
        record_helper_result(hub, &machine, *result)?;
        super::tell(hub, None);
        return Ok(Told::NetworkUpdates { reply: ViewReply::Done });
    }
    let (machines, requested, cancel) = match call {
        ViewCall::Repair { machines, version } => (machines, version, false),
        ViewCall::CancelRepair { machines } => (machines, None, true),
        _ => return Err("not a repair arrangement".into()),
    };
    let mut rows = std::collections::HashMap::new();
    for row in &view.machines {
        rows.entry(&row.machine).or_insert(row);
    }
    let mut manifests = std::collections::BTreeMap::new();
    let mut pending = Vec::new();
    let mut results = Vec::with_capacity(machines.len());
    for machine in machines {
        let result = async {
            let row = rows.get(&machine).copied().ok_or("this account cannot manage that machine")?;
            if !cancel && !row.actions.repair.as_ref().is_some_and(|permission| permission.allowed) {
                return Err("this machine does not support a repair arrangement".into());
            }
            let chosen = if cancel {
                None
            } else {
                let minimum = row
                    .kernel
                    .iter()
                    .chain(row.app.iter().map(|a| &a.version))
                    .chain(row.stage.version().map(str::to_string).as_ref())
                    .chain(row.due.iter().map(|d| &d.version))
                    .max_by_key(|v| release(v))
                    .cloned()
                    .ok_or("the machine's version is unknown")?;
                let version = if let Some(version) = &requested {
                    if release(version).is_none() || release(version) < release(&minimum) {
                        return Err("repair cannot go back to an older version".into());
                    }
                    version.clone()
                } else if row.repair_supported {
                    minimum.clone()
                } else {
                    view.releases
                        .iter()
                        .filter(|m| {
                            release(&m.version) >= release(REPAIR_SINCE)
                                && release(&m.version) >= release(&minimum)
                                && Channel::of(&minimum).takes(&m.version)
                                && release(&m.version).map(|v| v.major) == release(&minimum).map(|v| v.major)
                        })
                        .max_by_key(|m| m.semver())
                        .ok_or("no available release contains the repair program; this machine's version is too old")?
                        .version
                        .clone()
                };
                if !row.repair_supported && release(&version) < release(REPAIR_SINCE) {
                    return Err("this release is too old to contain the repair program".into());
                }
                if !manifests.contains_key(&version) {
                    super::view::read_manifest(hub, &version).await?;
                    let manifest = hub
                        .updates
                        .lock()
                        .expect("repair manifest")
                        .manifest(&version)
                        .ok_or("repair manifest is unavailable")?;
                    let cache = hub.updates.lock().expect("repair manifest cache").cache.clone();
                    let keeping = manifest.clone();
                    tokio::task::spawn_blocking(move || cache.keep_manifest(&keeping))
                        .await
                        .map_err(|e| e.to_string())??;
                    manifests.insert(version.clone(), Arc::new(manifest));
                }
                let manifest = manifests.get(&version).expect("prepared above").clone();
                if row.target.as_ref().is_some_and(|target| manifest.package(target).is_none()) {
                    return Err("repair release has no package for this machine".into());
                }
                Some(manifest)
            };
            Ok(chosen)
        }
        .await;
        results.push(match result {
            Ok(chosen) => {
                pending.push((results.len(), machine.clone(), chosen));
                Scheduled { machine, done: false, reason: None, why: None }
            }
            Err(why) => Scheduled { machine, done: false, reason: Some(ActionReason::Other), why: Some(why) },
        });
    }
    for chunk in pending.chunks(64) {
        let committed = super::repair_batch::apply(hub, asker, account, chunk);
        for ((at, _, _), result) in chunk.iter().zip(committed) {
            match result {
                Ok(()) => results[*at].done = true,
                Err(why) => {
                    results[*at].reason = Some(ActionReason::Other);
                    results[*at].why = Some(why);
                }
            }
        }
        tokio::task::yield_now().await;
    }
    super::prefetch(hub);
    super::tell(hub, None);
    super::prune(hub);
    Ok(Told::NetworkUpdates { reply: ViewReply::Scheduled { results } })
}

pub(super) fn settle(hub: &Hub, machine: &MachineName, result: &fastctx_types::repair::Report) {
    if result.finished.is_none() {
        return;
    }
    let mut store = hub.store();
    let Some(key) = store.book.directory.member(machine).map(|m| m.key) else { return };
    let Some(index) = store.hub.repairs.iter().position(|r| {
        r.key == key
            && r.due.id == result.id
            && release(&result.version).is_some()
            && release(&result.version) >= release(&r.due.version)
    }) else {
        return;
    };
    let before = store.hub.repairs.clone();
    if result.success {
        store.hub.repairs.remove(index);
    } else {
        let due = &mut std::sync::Arc::make_mut(&mut store.hub.repairs[index]).due;
        if due.observed == Some(result.started) {
            return;
        }
        due.observed = Some(result.started);
        due.problem = Some(Problem::new(
            FailureReason::Other,
            result.error.clone().unwrap_or_else(|| "repair did not complete".into()),
        ));
        let mut retry = fastctx_update::retry::Retry { attempts: due.attempts, next: due.retry_at };
        retry.postpone(jiff::Timestamp::now());
        due.attempts = retry.attempts;
        due.retry_at = retry.next;
    }
    if let Err(error) = store.save_hub() {
        store.hub.repairs = before;
        hub.log.line(format_args!("saving repair result for {machine}: {error}"));
    }
}

fn stage(report: &fastctx_types::repair::Report) -> Stage {
    if report.finished.is_none() {
        Stage::Repairing {
            version: report.version.clone(),
            layer: report.steps.last().map(|s| s.layer.clone()).unwrap_or_else(|| "package".into()),
        }
    } else if !report.success {
        Stage::Failed {
            version: report.version.clone(),
            problem: Problem::from(report.error.clone().unwrap_or_else(|| "repair did not complete".into())),
        }
    } else {
        Stage::Idle
    }
}

fn record_helper_result(hub: &Hub, machine: &MachineName, report: fastctx_types::repair::Report) -> Result<(), String> {
    let key = hub.store().book.directory.member(machine).ok_or("repair target disappeared")?.key;
    settle(hub, machine, &report);
    let mut updates = hub.updates.lock().expect("repair result");
    let last = updates.last.get_mut(&key).ok_or("the legacy machine has never reported its version")?;
    let last = std::sync::Arc::make_mut(last);
    last.report.stage = stage(&report);
    last.report.repair = Some(report.clone());
    if let Some((_, active)) = updates.reports.get_mut(machine) {
        active.stage = stage(&report);
        active.repair = Some(report);
    }
    updates.history_writer.put(key, updates.last.get(&key).cloned());
    Ok(())
}

/// Old nodes cannot echo a new repair field, so their ordinary version report
/// preserves the receipt independently obtained by the authorized helper.
pub(super) fn merge_legacy_report(hub: &Hub, machine: &MachineName, report: &mut fastctx_types::update::Report) {
    if report.repair_supported || report.repair.is_some() {
        return;
    }
    let Some(key) = hub.store().book.directory.member(machine).map(|m| m.key) else { return };
    if let Some(repair) =
        hub.updates.lock().expect("legacy repair receipt").last.get(&key).and_then(|r| r.report.repair.clone())
    {
        report.stage = stage(&repair);
        report.repair = Some(repair);
    }
}
