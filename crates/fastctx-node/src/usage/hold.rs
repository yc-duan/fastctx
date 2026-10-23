//! Publishes request holds and stops affected main turns outside report locks.
use super::allowance::{self, UsedUp};
use crate::Node;
use fastctx_types::ResidentId;
use fastctx_types::residents::About;
use fastctx_types::session::{Health, HealthKind};
use jiff::Timestamp;
use serde_json::json;
use std::path::Path;
use std::sync::Arc;

fn publish(
    node: &Node,
    about: &About,
    restriction: Option<&(fastctx_types::access::HubAccount, UsedUp)>,
) -> std::io::Result<()> {
    if about.host.agent.as_str() != "claude" {
        return Ok(());
    }
    let Some(session) = &about.session else { return Ok(()) };
    let directory = fastctx_hosts::claude::exchange::sessions(node.world.path()).join(session);
    if !directory.is_dir() {
        return Ok(());
    }
    let path = directory.join("hold.json");
    let desired = restriction.map(|(account, used)| json!({"until": used.expires(), "said": used.said(account), "models": if used.fable() { Some(["fable"]) } else { None }}));
    match desired {
        Some(value) => {
            let bytes = serde_json::to_vec(&value).map_err(std::io::Error::other)?;
            if std::fs::read(&path).ok().as_ref() != Some(&bytes) {
                fastctx_platform::publish(&path, &bytes, fastctx_platform::Permissions::Private)?;
            }
        }
        None => match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        },
    }
    Ok(())
}

pub(super) fn refresh(node: &Arc<Node>, login: Option<(&str, &Path)>) -> Option<Timestamp> {
    let _refreshing = node.usage.enforcing.lock().expect("never poisoned");
    let records: Vec<(ResidentId, About)> = node
        .residents
        .everyone()
        .filter(|record| record.removed.is_none())
        .filter(|record| {
            login.is_none_or(|(agent, dir)| {
                record.about.host.agent.as_str() == agent
                    && allowance::login(node, &record.about).is_some_and(|home| crate::homes::same_dir(&home, dir))
            })
        })
        .map(|record| (record.id, record.about))
        .collect();
    let mut next = None;
    for (id, about) in records {
        let restriction = allowance::restriction(node, &about);
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
        tokio::spawn(async move {
            // 2026-10-20: a report holds its session gate; stopping another session must
            // never wait beneath that gate (world-concurrency.md).
            if node.residents.now(&id).is_some_and(|now| now.turn.as_ref() == Some(&turn)) {
                if let Err(failure) = crate::talk::stop_here_as(&node, &id, fastctx_types::turn_evidence::StopSource::Allowance).await {
                    node.log.line(format_args!("stopping {id} at its allowance: {}", failure.message));
                }
                let health = Health { kind: HealthKind::Warning, text: used.said(&account) };
                if let Err(error) = crate::turns::health(&node, &id, None, health) {
                    node.log.line(format_args!("recording {id}'s usage hold: {error}"));
                }
            }
            node.usage.stopping.lock().expect("never poisoned").remove(&key);
        });
    }
    next
}

/// Rechecks on hub-view changes, and at an active hold's expiry. The periodic
/// pass also covers a new session registered while no usage report was arriving.
pub(crate) fn watch(node: &Arc<Node>) {
    let mut views = node.views.subscribe();
    let weak = Arc::downgrade(node);
    tokio::spawn(async move {
        loop {
            let delay = {
                let Some(node) = weak.upgrade() else { return };
                let until = refresh(&node, None);
                until
                    .map(|until| until.duration_since(Timestamp::now()).as_millis().clamp(1, 30_000) as u64)
                    .unwrap_or(30_000)
            };
            tokio::select! {
                result = views.changed() => if result.is_err() { return; },
                _ = tokio::time::sleep(std::time::Duration::from_millis(delay)) => {},
            }
        }
    });
}
