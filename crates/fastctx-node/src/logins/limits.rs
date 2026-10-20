//! Codex's limits as its app-servers read them or push them while a login's
//! sessions run, kept with when they came (`world-agent-logins.md`, 额度):
//! a push is merged into what was last read, as Codex asks. A fresh read is
//! made only when the user asks for one.

use std::path::Path;
use std::sync::Arc;

use fastctx_hosts::codex::{LimitsHow, LimitsReport};
use fastctx_types::AgentName;
use fastctx_types::agents::{AgentLimits, Fact};
use fastctx_types::ipc::Failure;
use tokio::sync::mpsc;

use super::kept::{self, KeptLimits};
use crate::Node;

/// Keeps every report as it comes; one that changed the login's windows
/// can stop a turn on it at an assistance's allowance.
pub(crate) fn listen(node: Arc<Node>, mut reports: mpsc::UnboundedReceiver<LimitsReport>) {
    tokio::spawn(async move {
        while let Some(report) = reports.recv().await {
            let (keeping, home) = (node.clone(), report.home.clone());
            let kept = tokio::task::spawn_blocking(move || keep(&keeping, report)).await;
            match kept {
                Ok(Ok(true)) => crate::usage::after_reading(&node, "codex", &home).await,
                Ok(Ok(false)) => {}
                Ok(Err(error)) => node.log.line(format_args!("keeping Codex's limits: {error}")),
                Err(error) => node.log.line(format_args!("keeping Codex's limits: {error}")),
            }
        }
    });
}

/// Records one report of a Codex login's limits, and keeps it as a reading
/// of the login's windows in its usage ledger. Whether that reading changed
/// them.
pub(super) fn keep(node: &Node, report: LimitsReport) -> std::io::Result<bool> {
    let (home, at) = (report.home.clone(), report.at);
    let _keeping = node.logins.keeping.lock().expect("never poisoned");
    let mut kept = kept::read(node)?;
    {
        let limits = &mut kept.codex.limits;
        let index = limits.iter().position(|known| crate::homes::same_dir(&known.dir, &report.home));
        match report.how {
            LimitsHow::Read(answer) => {
                let buckets = fastctx_hosts::codex::buckets(&answer);
                let fresh = KeptLimits { dir: report.home.clone(), at: report.at, buckets };
                match index {
                    Some(index) => limits[index] = fresh,
                    None => limits.push(fresh),
                }
            }
            LimitsHow::Updated(update) => {
                let entry = match index {
                    Some(index) => &mut limits[index],
                    None => {
                        limits.push(KeptLimits {
                            dir: report.home.clone(),
                            at: report.at,
                            buckets: Default::default(),
                        });
                        limits.last_mut().expect("just pushed")
                    }
                };
                fastctx_hosts::codex::merge(&mut entry.buckets, &update);
                entry.at = report.at;
            }
        }
    }
    // 2026-10-20: publish credits only after the matching windows are committed,
    // so a reader of the new snapshot cannot see the previous quota period.
    let changed = match raw_of(&kept, &home) {
        Fact::Known { value } => crate::usage::codex_reading(node, &home, value.windows, at)?,
        _ => false,
    };
    kept::write(node, &kept)?;
    Ok(changed)
}

/// The limits of the Codex login in `dir` as they were last read or pushed.
pub(crate) fn kept_of(node: &Node, dir: &Path) -> std::io::Result<Fact<AgentLimits>> {
    Ok(of(node, &kept::read(node)?, dir))
}

/// The kept limits of the Codex login in `dir`.
pub(super) fn of(node: &Node, kept: &kept::Kept, dir: &Path) -> Fact<AgentLimits> {
    let raw = raw_of(kept, dir);
    let Some(mut merged) = crate::usage::last_limits(node, "codex", dir) else { return raw };
    if let Fact::Known { value } = raw {
        merged.credits = value.credits;
    }
    Fact::known(merged)
}

fn raw_of(kept: &kept::Kept, dir: &Path) -> Fact<AgentLimits> {
    match kept.codex.limits.iter().find(|known| crate::homes::same_dir(&known.dir, dir)) {
        Some(known) => fastctx_hosts::codex::limits_of(&known.buckets, known.at),
        None => Fact::unavailable("not read yet"),
    }
}

/// Reads a login's limits afresh: Codex's through its app-server, the
/// running one or one started for the reading; Claude Code reads its own
/// only in a session at work, so nothing can be asked of it.
pub(crate) async fn read(node: &Arc<Node>, agent: &AgentName, dir: &Path) -> Result<(), Failure> {
    {
        let (node, agent, dir) = (node.clone(), agent.clone(), dir.to_path_buf());
        tokio::task::spawn_blocking(move || super::known(&node, &agent, &dir))
            .await
            .map_err(|error| Failure::new(error.to_string()))??;
    }
    if agent.as_str() != "codex" {
        return Err(Failure::not_run(
            "Claude Code reads its limits only in a session at work; they are shown as its sessions last read them",
        ));
    }
    let asked = super::identify::codex(node, dir, true).await;
    let Some((at, answer)) = asked.limits else {
        return Err(Failure::new(match asked.identified.who {
            Fact::Known { value } if !value.logged_in => format!("no one is logged in to {}", dir.display()),
            Fact::Known { .. } => format!("Codex read no limits for {}", dir.display()),
            Fact::Unavailable { reason } | Fact::Failed { reason } => reason,
        }));
    };
    let (keeping, home) = (node.clone(), dir.to_path_buf());
    let changed = tokio::task::spawn_blocking(move || keep(&keeping, super::identify::limits_read(&home, at, answer)))
        .await
        .map_err(|error| Failure::new(error.to_string()))?
        .map_err(|error| Failure::new(format!("keeping Codex's limits: {error}")))?;
    if changed {
        crate::usage::after_reading(node, "codex", dir).await;
    }
    Ok(())
}
