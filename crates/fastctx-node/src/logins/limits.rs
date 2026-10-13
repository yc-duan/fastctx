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

/// Keeps every report as it comes.
pub(crate) fn listen(node: Arc<Node>, mut reports: mpsc::UnboundedReceiver<LimitsReport>) {
    tokio::spawn(async move {
        while let Some(report) = reports.recv().await {
            let keeping = node.clone();
            let kept = tokio::task::spawn_blocking(move || keep(&keeping, report)).await;
            match kept {
                Ok(Ok(())) => {}
                Ok(Err(error)) => node.log.line(format_args!("keeping Codex's limits: {error}")),
                Err(error) => node.log.line(format_args!("keeping Codex's limits: {error}")),
            }
        }
    });
}

/// Records one report of a Codex login's limits, and keeps it as a reading
/// of the login's windows in its usage ledger.
pub(super) fn keep(node: &Node, report: LimitsReport) -> std::io::Result<()> {
    let (home, at) = (report.home.clone(), report.at);
    kept::change(node, |kept| {
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
    })?;
    if let Fact::Known { value } = kept_of(node, &home)? {
        crate::usage::codex_reading(node, &home, value.windows, at);
    }
    Ok(())
}

/// The limits of the Codex login in `dir` as they were last read or pushed.
pub(crate) fn kept_of(node: &Node, dir: &Path) -> std::io::Result<Fact<AgentLimits>> {
    Ok(of(&kept::read(node)?, dir))
}

/// The kept limits of the Codex login in `dir`.
pub(super) fn of(kept: &kept::Kept, dir: &Path) -> Fact<AgentLimits> {
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
    let node = node.clone();
    let dir = dir.to_path_buf();
    tokio::task::spawn_blocking(move || keep(&node, super::identify::limits_read(&dir, at, answer)))
        .await
        .map_err(|error| Failure::new(error.to_string()))?
        .map_err(|error| Failure::new(format!("keeping Codex's limits: {error}")))
}
