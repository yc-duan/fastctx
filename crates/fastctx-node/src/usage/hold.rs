//! Publishes request holds and stops affected main turns outside report locks.
use super::allowance::{self, UsedUp};
use crate::Node;
use fastctx_types::ResidentId;
use fastctx_types::access::Acting;
use fastctx_types::residents::About;
use fastctx_types::session::{Health, HealthKind};
use jiff::Timestamp;
use serde_json::json;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

fn publish(
    node: &Node,
    about: &About,
    restriction: Option<&(Option<fastctx_types::access::HubAccount>, UsedUp)>,
) -> std::io::Result<()> {
    if about.host.agent.as_str() != "claude" {
        return Ok(());
    }
    let Some(session) = &about.session else { return Ok(()) };
    let directory = fastctx_hosts::claude::exchange::sessions(node.world.path()).join(session);
    if !fastctx_platform::disk::is_dir(&directory) {
        return Ok(());
    }
    let path = directory.join("hold.json");
    let desired = restriction.map(|(account, used)| json!({"until": used.expires(), "said": used.said(account.as_ref()), "models": if used.fable() { Some(["fable"]) } else { None }}));
    match desired {
        Some(value) => {
            let bytes = serde_json::to_vec(&value).map_err(std::io::Error::other)?;
            if fastctx_platform::disk::read(&path).ok().as_ref() != Some(&bytes) {
                fastctx_platform::publish(&path, &bytes, fastctx_platform::Permissions::Private)?;
            }
        }
        None => match fastctx_platform::disk::remove_file(path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        },
    }
    Ok(())
}

/// Checks the conversations that work for accounts in `login`, or in every
/// login, each login under its own lock (`world-usage.md`, 守线): only they
/// can be held, and one login's check waits for no other's.
pub(super) fn refresh(node: &Arc<Node>, login: Option<(&str, &Path)>) -> Option<Timestamp> {
    let defaults = (node.codex.default().config_home().to_path_buf(), super::claude_default());
    let mut logins: BTreeMap<String, Vec<(ResidentId, About)>> = BTreeMap::new();
    for record in node.residents.everyone() {
        let agent = record.about.host.agent.as_str();
        // Those working for accounts may be held by their assistance; the
        // machine's own Claude Code conversations by the login's Fable line.
        if record.removed.is_some() || !(matches!(record.about.acting, Acting::Accounts { .. }) || agent == "claude") {
            continue;
        }
        let dir = match (&record.about.home, agent) {
            (Some(home), _) => home.clone(),
            (None, "codex") => defaults.0.clone(),
            (None, "claude") => match &defaults.1 {
                Some(dir) => dir.clone(),
                None => continue,
            },
            _ => continue,
        };
        if login.is_some_and(|(wanted, wanted_dir)| agent != wanted || !crate::homes::same_dir(&dir, wanted_dir)) {
            continue;
        }
        logins.entry(format!("{agent}-{}", crate::homes::key(&dir))).or_default().push((record.id, record.about));
    }
    let mut next = None;
    for (key, records) in logins {
        let gate = node.usage.enforcing.lock().expect("never poisoned").entry(key).or_default().clone();
        let _refreshing = gate.lock().expect("never poisoned");
        if let Some(until) = refresh_login(node, records) {
            next = Some(next.map_or(until, |old: Timestamp| old.min(until)));
        }
    }
    next
}

/// Publishes each conversation's hold and stops the turns past a limit;
/// says when the first hold ends.
fn refresh_login(node: &Arc<Node>, records: Vec<(ResidentId, About)>) -> Option<Timestamp> {
    let mut next = None;
    let mut restrictions = allowance::Restrictions::default();
    for (id, about) in records {
        let restriction = restrictions.of(node, &about);
        if let Err(error) = publish(node, &about, restriction.as_ref()) {
            node.log.line(format_args!("publishing {id}'s usage hold: {error}"));
        }
        let Some((account, used)) = restriction else { continue };
        next = Some(next.map_or(used.until(), |old: Timestamp| old.min(used.until())));
        if !used.applies(about.model.as_deref()) {
            continue;
        }
        let Some(turn) = node.residents.now(&id).and_then(|now| now.turn) else { continue };
        let key = (id.clone(), turn.clone());
        if !node.usage.stopping.lock().expect("never poisoned").insert(key.clone()) {
            continue;
        }
        let node = node.clone();
        fastctx_platform::task::spawn(async move {
            // A report holds its session gate; stopping another session must
            // never wait beneath that gate (world-concurrency.md).
            if node.residents.now(&id).is_some_and(|now| now.turn.as_ref() == Some(&turn)) {
                if let Err(failure) =
                    crate::talk::stop_here_as(&node, &id, fastctx_types::turn_evidence::StopSource::Allowance).await
                {
                    node.log.line(format_args!("stopping {id} at its allowance: {}", failure.message));
                }
                let health = Health { kind: HealthKind::Warning, text: used.said(account.as_ref()) };
                if let Err(error) = crate::turns::health(&node, &id, None, health) {
                    node.log.line(format_args!("recording {id}'s usage hold: {error}"));
                }
            }
            node.usage.stopping.lock().expect("never poisoned").remove(&key);
        });
    }
    next
}

/// How long after the hub's view changed the holds are checked again: views
/// change in bursts as machines come and go, and each check reads every
/// conversation here.
const VIEWS_SETTLE: std::time::Duration = std::time::Duration::from_secs(5);

/// The longest the holds go unchecked with nothing told: a safety net.
const QUIET: std::time::Duration = std::time::Duration::from_secs(10 * 60);

/// Rechecks on hub-view changes, at an active hold's expiry, and when a
/// conversation is born here or its session is bound (one that no usage
/// report has reached yet must still be held: its `hold.json` goes with its
/// session; usage reports check their own login, `allowance`). Each check
/// goes through every conversation here: heavy work, done on a thread that
/// may block, and never on a timer with nothing to look at.
pub(crate) fn watch(node: &Arc<Node>) {
    let mut views = node.views.subscribe();
    let mut residents = node.residents.changes();
    let weak = Arc::downgrade(node);
    fastctx_platform::task::spawn(async move {
        loop {
            let (delay, mut known, mut seen) = {
                let Some(node) = weak.upgrade() else { return };
                // Where the residents stand as the check begins: one born or
                // bound since is looked at by the next.
                let seen = node.residents.changed_since(u64::MAX).at;
                let known: std::collections::HashMap<ResidentId, bool> = node
                    .residents
                    .ids()
                    .into_iter()
                    .map(|id| {
                        let bound = node.residents.about(&id).is_some_and(|(about, _)| about.session.is_some());
                        (id, bound)
                    })
                    .collect();
                let until = fastctx_platform::task::blocking(move || refresh(&node, None)).await;
                let delay = until.map_or(QUIET, |until| {
                    let millis = until.duration_since(Timestamp::now()).as_millis().clamp(1, QUIET.as_millis() as i128);
                    std::time::Duration::from_millis(millis as u64)
                });
                (delay, known, seen)
            };
            let deadline = tokio::time::Instant::now() + delay;
            loop {
                tokio::select! {
                    result = views.changed() => {
                        if result.is_err() {
                            return;
                        }
                        tokio::time::sleep(VIEWS_SETTLE).await;
                        views.borrow_and_update();
                        break;
                    }
                    result = residents.changed() => {
                        if result.is_err() {
                            return;
                        }
                        let Some(node) = weak.upgrade() else { return };
                        let changed = node.residents.changed_since(seen);
                        seen = changed.at;
                        if changed.everyone {
                            break;
                        }
                        let mut anew = false;
                        for id in changed.residents {
                            let bound = node.residents.about(&id).is_some_and(|(about, _)| about.session.is_some());
                            match known.insert(id, bound) {
                                None => anew = true,
                                Some(was) => anew |= bound && !was,
                            }
                        }
                        if anew {
                            break;
                        }
                    }
                    () = tokio::time::sleep_until(deadline) => break,
                }
            }
        }
    });
}
