//! One durable launch per resident, shared by prompt and control resumption.

use super::wake_store::{self, Intent};
use crate::Node;
use fastctx_hosts::claude::{background, exchange};
use fastctx_jobs::{Kind, Outcome, Taken};
use fastctx_types::{ResidentId, ipc::Failure};
use std::{future::Future, path::Path, pin::Pin, sync::Arc, time::Duration};

pub(super) const KIND: &str = "claude/reconcile-wake";
// The CLI has a 120-second deadline. A missing receipt additionally needs a
// continuous minute of unchanged terminal evidence after that deadline.
const QUIET: Duration = Duration::from_secs(180);
const AGAIN: Duration = Duration::from_secs(3);

pub(super) struct Observation {
    pub running: bool,
    pub resting: bool,
    terminal: bool,
    evidence: String,
}

/// All supervisor predicates use the same CLI answer. A starting job without
/// a PID is still in flight; absence from the list does not establish failure.
pub(super) async fn observe(node: &Arc<Node>, home: Option<&Path>, session: &str) -> Result<Observation, Failure> {
    let listed = super::management::run(node, home, vec!["agents".into(), "--json".into(), "--all".into()]).await?;
    let running = background::running(&listed, session).map_err(Failure::new)?;
    let resting = background::resting(&listed, session).map_err(Failure::new)?;
    let rows: Vec<serde_json::Value> =
        serde_json::from_str(&listed).map_err(|error| Failure::new(error.to_string()))?;
    let named: Vec<_> = rows
        .iter()
        .filter(|row| {
            row["sessionId"].as_str() == Some(session)
                || row["id"].as_str() == Some(session)
                || row["sessionId"].as_str().is_some_and(|full| session.len() == 8 && full.starts_with(session))
        })
        .collect();
    let directory = exchange::sessions(node.world.path()).join(session);
    let registration = exchange::registration(&directory).map_err(|error| Failure::new(error.to_string()))?;
    let present = registration
        .as_ref()
        .map(|registration| super::pairing::present(node, registration))
        .transpose()
        .map_err(|error| Failure::new(error.to_string()))?
        .unwrap_or(false);
    let transcript = exchange::transcript(&directory).map_err(|error| Failure::new(error.to_string()))?;
    let length = transcript
        .map(|path| {
            fastctx_platform::disk::metadata(path).and_then(|metadata| {
                let changed = metadata
                    .modified()?
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_err(std::io::Error::other)?
                    .as_nanos();
                Ok((metadata.len(), changed))
            })
        })
        .transpose()
        .map_err(|error| Failure::new(error.to_string()))?;
    let settled_blocked = if named.iter().any(|row| row["state"] == "blocked") {
        let command = super::command(node, home)?;
        super::startup::settled(&command, session.get(..8).unwrap_or(session))
    } else {
        false
    };
    let terminal = named.iter().all(|row| {
        row["kind"].as_str() == Some("background")
            && (matches!(row["state"].as_str(), Some("done" | "failed" | "stopped"))
                || (row["state"] == "blocked" && settled_blocked))
            && row["pid"].as_u64().is_none()
    });
    let evidence =
        serde_json::to_string(&(named, registration, length)).map_err(|error| Failure::new(error.to_string()))?;
    Ok(Observation { running: running || present, resting: resting && !present && terminal, terminal, evidence })
}

pub(super) enum Admission {
    Launch(Box<Launch>),
    Present,
    Waiting,
}

pub(super) struct Launch {
    pub intent: Intent,
    pub active: crate::leases::Lease<ResidentId, ()>,
}

pub(super) fn hold(node: &Node, id: &ResidentId) -> Option<crate::leases::Lease<ResidentId, ()>> {
    node.claude.launches.get(id)
}

/// Returns permission only after the unique durable intent has committed.
pub(super) async fn begin(
    node: &Arc<Node>,
    id: &ResidentId,
    home: Option<&Path>,
    session: &str,
) -> Result<Admission, Failure> {
    let prior = wake_store::get(node, id)?;
    let expected = prior.as_ref().map(|intent| intent.token.clone());
    let seen = observe(node, home, session).await?;
    if let Some(intent) = prior
        && !reconcile(node, id, intent, &seen)?
    {
        return Ok(Admission::Waiting);
    }
    if seen.running {
        return Ok(Admission::Present);
    }
    if hold(node, id).is_some() || !seen.resting {
        return Ok(Admission::Waiting);
    }
    let intent = Intent {
        token: crate::speech::new_id("wake").to_string(),
        revision: 0,
        session: session.to_string(),
        owner: fastctx_platform::current_identity().map_err(|error| Failure::new(error.to_string()))?,
        before: seen.evidence.clone(),
        accepted: false,
        completed: false,
        settled: false,
        quiet_since: None,
        quiet_owner: None,
        last_seen: None,
        last: seen.evidence,
        problem: None,
    };
    let active = node.claude.launches.get_or_insert_with(id, || ());
    if !wake_store::claim(node, id, intent.clone(), expected)? {
        return Ok(Admission::Waiting);
    }
    node.durable_jobs.stir(KIND);
    Ok(Admission::Launch(Box::new(Launch { intent, active })))
}

/// A fresh live activation settles a launch. A stale stopped row can settle a
/// lost receipt only after uninterrupted, unchanged terminal observations.
fn reconcile(node: &Node, id: &ResidentId, mut intent: Intent, seen: &Observation) -> Result<bool, Failure> {
    if intent.settled {
        return Ok(true);
    }
    let before = activation(&intent.before)?;
    let current = activation(&seen.evidence)?;
    // A worker can register and finish between observations. Its durable
    // activation identity still proves that this launch crossed startup.
    let registered = current.is_some() && current != before;
    if registered || (seen.running && seen.evidence != intent.before) {
        return wake_store::settle_seen(node, id, &intent);
    }
    // A slow preparation or a queued blocking launcher may still issue the
    // CLI. Only abandoned calls, or calls that have finished, can time out.
    if hold(node, id).is_some() {
        intent.quiet_since = None;
        wake_store::put(node, id, intent)?;
        return Ok(false);
    }
    if seen.resting && seen.terminal {
        let now = observation_time()?;
        let owner = fastctx_platform::current_identity().map_err(|error| Failure::new(error.to_string()))?;
        let interrupted = intent.quiet_owner.as_ref() != Some(&owner)
            || intent.last_seen.is_none_or(|last| now.as_second().saturating_sub(last.as_second()) > 60);
        if interrupted || intent.last != seen.evidence || intent.quiet_since.is_none() {
            intent.quiet_since = Some(now);
            intent.quiet_owner = Some(owner);
        }
        intent.last_seen = Some(now);
        if intent
            .quiet_since
            .is_some_and(|since| now.as_second().saturating_sub(since.as_second()) >= QUIET.as_secs() as i64)
        {
            if !wake_store::settle_seen(node, id, &intent)? {
                return Ok(false);
            }
            node.log.line(format_args!("Claude wake {id} {} reconciled as not started: 180 seconds of unchanged terminal or absent supervisor records, no registered host process, and no transcript growth", intent.token));
            return Ok(true);
        }
    } else {
        intent.quiet_since = None;
    }
    intent.problem = None;
    intent.last.clone_from(&seen.evidence);
    wake_store::put(node, id, intent)?;
    Ok(false)
}

/// Clock adjustments cannot shorten an unresolved launch's observation window.
fn observation_time() -> Result<jiff::Timestamp, Failure> {
    static CLOCK: std::sync::LazyLock<(jiff::Timestamp, std::time::Instant)> =
        std::sync::LazyLock::new(|| (jiff::Timestamp::now(), std::time::Instant::now()));
    // The observer identity resets the interval after a node restart, so
    // persisted quiet intervals never cross this process's clock anchor.
    CLOCK.0.checked_add(CLOCK.1.elapsed()).map_err(|error| Failure::new(error.to_string()))
}

fn activation(evidence: &str) -> Result<Option<serde_json::Value>, Failure> {
    let snapshot: serde_json::Value = serde_json::from_str(evidence)
        .map_err(|error| Failure::new(format!("reading Claude wake observation: {error}")))?;
    let registration = &snapshot[1];
    let identity = serde_json::json!([registration["instance"], registration["process"]]);
    Ok((!registration["instance"].is_null() || !registration["process"].is_null()).then_some(identity))
}

/// Record every CLI outcome before callers may release their transient guards.
/// A process-boundary failure keeps the intent for reconciliation.
pub(super) fn finish(
    node: &Node,
    id: &ResidentId,
    intent: Intent,
    started: bool,
    accepted: bool,
) -> Result<(), Failure> {
    if !started {
        return wake_store::clear(node, id, &intent.token);
    }
    wake_store::completed(node, id, &intent.token, accepted)
}

pub(super) fn waiting() -> Failure {
    Failure::host_unavailable("a Claude wake is already in flight; waiting for its activation or reconciled failure")
}

pub(crate) fn register(node: &Arc<Node>) {
    let weak = Arc::downgrade(node);
    let run = move |taken: Taken| -> Pin<Box<dyn Future<Output = Outcome> + Send>> {
        let node = weak.upgrade();
        Box::pin(async move {
            let Some(node) = node else { return Outcome::Failed("the node is stopping".into()) };
            let Ok(id) = taken.key.parse::<ResidentId>() else { return Outcome::Done };
            node.ready_for(&id).await;
            let result = async {
                let Some(intent) = wake_store::get(&node, &id)? else { return Ok(true) };
                if intent.settled {
                    return Ok(true);
                }
                let Some((about, _)) = node.residents.about(&id) else { return Ok(true) };
                let session = about.session.as_deref().unwrap_or(&intent.session);
                let seen = observe(&node, about.home.as_deref(), session).await?;
                reconcile(&node, &id, intent, &seen)
            }
            .await;
            match result {
                Ok(true) => {
                    crate::lifecycle::retry::kick(&node, &id);
                    Outcome::Done
                }
                Ok(false) => Outcome::Again { after: AGAIN },
                Err(error) => {
                    if let Ok(Some(mut intent)) = wake_store::get(&node, &id) {
                        if intent.problem.as_deref() != Some(&error.message) {
                            node.log.line(format_args!(
                                "Claude wake {id}: reconciliation still pending: {}",
                                error.message
                            ));
                        }
                        intent.problem = Some(error.message);
                        intent.quiet_since = None;
                        let _ = wake_store::put(&node, &id, intent);
                    }
                    Outcome::Again { after: AGAIN }
                }
            }
        })
    };
    node.durable_jobs
        .register(Kind { name: KIND, at_once: 32, within: Duration::from_secs(120), tries: 5 }, Arc::new(run));
    super::copies::register(node);
}
