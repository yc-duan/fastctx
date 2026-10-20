//! Recover an idle activation before queued input crosses the host boundary.

use std::io;
use std::sync::Arc;

use fastctx_hosts::claude::{control, startup_hook};
use fastctx_platform::{Permissions, publish};
use fastctx_types::residents::{Activity, PendingState, Trouble};
use fastctx_types::session::{Health, HealthKind};
use fastctx_types::{NoticeId, ResidentId};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::Node;

// A native receipt normally follows mod startup. A missing receipt has its
// own deadline, independent of the frequency at which startup is observed.
const PAIRING_SECONDS: i64 = 120;

#[derive(Deserialize, Serialize)]
struct Attempt {
    session: String,
    input: NoticeId,
    at: Timestamp,
    outcome: Outcome,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Outcome {
    Requested,
    Accepted,
    Failed(String),
    Unpaired,
}

/// Returns true after a restart request, so the observer discards its old
/// supervisor snapshot. A durable intent prevents repeats after node restart.
pub(super) async fn observe(
    node: &Arc<Node>,
    resident: &ResidentId,
    id: &NoticeId,
    short: &str,
    command: &super::Command,
) -> io::Result<bool> {
    let directory = node.world.conversation(resident);
    let birth: Option<startup_hook::Opening> = control::read(&startup_hook::opening(&directory))?;
    let Some(birth) = birth.filter(|birth| birth.input == *id) else { return Ok(false) };
    let Some(first) = crate::inbox::read(&directory, id)? else { return Ok(false) };
    if first.state != PendingState::Waiting || super::startup::ready(node, resident)? {
        return Ok(false);
    }
    let path = directory.path().join("claude-startup-recovery.json");
    let attempt =
        control::read::<Attempt>(&path)?.filter(|attempt| attempt.input == birth.input && attempt.session == short);
    let after = attempt.as_ref().map_or(birth.at, |attempt| attempt.at);
    let receipt: Option<startup_hook::Started> = control::read(&startup_hook::receipt(&directory))?;
    let now = Timestamp::now();
    let reached_start = match receipt {
        Some(receipt)
            if receipt.session.starts_with(short)
                && Some(&receipt.process) != birth.previous.as_ref()
                && (attempt.is_none() || receipt.at >= after)
                && now.duration_since(receipt.at).as_secs() >= 2 =>
        {
            fastctx_platform::is_alive(&receipt.process)?
        }
        _ => false,
    };
    if !reached_start && now.duration_since(after).as_secs() < PAIRING_SECONDS {
        return Ok(false);
    }
    if let Some(mut attempt) = attempt {
        if attempt.session == short && matches!(attempt.outcome, Outcome::Requested | Outcome::Accepted) {
            attempt.outcome = Outcome::Unpaired;
            publish(&path, &serde_json::to_vec(&attempt).map_err(io::Error::other)?, Permissions::Private)?;
            waiting(node, resident, "Claude's plugin still has not paired after an automatic restart; the queued words remain queued and have not been submitted".into())?;
        }
        return Ok(false);
    }
    // Replay before taking management: pairing itself shares this gate.
    super::replay(node).await;
    let _place = node.places.using(resident.as_str()).await;
    let Ok(gate) = node.residents.management(resident) else { return Ok(false) };
    let _exclusive = gate.write().await;
    if super::startup::ready(node, resident)?
        || node.residents.removed(resident).is_some()
        || node.residents.about(resident).is_none_or(|(about, _)| {
            !about.world_started || about.session.as_deref().is_none_or(|session| !session.starts_with(short))
        })
        || crate::inbox::read(&directory, id)?.is_none_or(|first| first.state != PendingState::Waiting)
    {
        return Ok(false);
    }
    let mut attempt =
        Attempt { session: short.into(), input: id.clone(), at: Timestamp::now(), outcome: Outcome::Requested };
    publish(&path, &serde_json::to_vec(&attempt).map_err(io::Error::other)?, Permissions::Private)?;
    let reason = if reached_start { "its native SessionStart hook ran" } else { "startup did not finish within 120 s" };
    let text = format!(
        "Claude background session {short} has no paired World plugin although {reason}; restarting this idle session once, with its queued words still queued"
    );
    node.log.line(&text);
    crate::turns::health(node, resident, None, Health { kind: HealthKind::Warning, text })?;
    let result = super::startup::run(command, vec!["respawn".into(), short.into()]).await;
    attempt.outcome = match result {
        Ok(_) => Outcome::Accepted,
        Err(error) => {
            waiting(
                node,
                resident,
                format!(
                    "Could not restart Claude background session {short}: {error}; the queued words remain queued and have not been submitted"
                ),
            )?;
            Outcome::Failed(error)
        }
    };
    publish(&path, &serde_json::to_vec(&attempt).map_err(io::Error::other)?, Permissions::Private)?;
    Ok(true)
}

/// A session woken outside message delivery can be alive without its mod.
/// Keep the input local and observe that activation too.
pub(super) fn watch_waiting(
    node: &Arc<Node>,
    resident: &ResidentId,
    session: &str,
    pending: &fastctx_types::residents::Pending,
) -> io::Result<bool> {
    let directory = node.world.conversation(resident);
    let opening: Option<startup_hook::Opening> = control::read(&startup_hook::opening(&directory))?;
    if opening.is_none() {
        // Legacy launches may already be running a native prompt. Their
        // missing mod is not evidence that restarting them is safe.
        waiting(node, resident, "Claude has no paired plugin and no idle-start receipt; its earlier input may already be running, so it was not restarted automatically".into())?;
        return Ok(false);
    }
    let retained = match &opening {
        Some(opening) => {
            crate::inbox::read(&directory, &opening.input)?.is_some_and(|input| input.state == PendingState::Waiting)
        }
        None => false,
    };
    let id = if retained {
        opening.expect("a retained opening").input
    } else {
        let registration = fastctx_hosts::claude::exchange::registration(
            &fastctx_hosts::claude::exchange::sessions(node.world.path()).join(session),
        )?;
        if let Some(registration) = &registration
            && registration.instance.is_some()
            && super::pairing::present(node, registration)?
        {
            return Ok(true);
        }
        let opening = startup_hook::Opening {
            input: pending.delivery.id.clone(),
            at: Timestamp::now(),
            previous: registration.and_then(|registration| registration.process),
        };
        publish(
            &startup_hook::opening(&directory),
            &serde_json::to_vec(&opening).map_err(io::Error::other)?,
            Permissions::Private,
        )?;
        opening.input
    };
    let Some((about, _)) = node.residents.about(resident) else { return Ok(false) };
    let command = super::command(node, about.home.as_deref()).map_err(|error| io::Error::other(error.message))?;
    super::startup::observe(node, resident, id, command);
    Ok(false)
}

/// Unsent input remains queued; it must never become an uncertain delivery.
pub(super) fn waiting(node: &Arc<Node>, resident: &ResidentId, text: String) -> io::Result<()> {
    node.log.line(&text);
    crate::turns::health(node, resident, None, Health { kind: HealthKind::Error, text: text.clone() })?;
    node.residents.change_now(&node.world, resident, |now| {
        if now.turn.is_none() {
            now.activity = Activity::Idle;
            now.since = Timestamp::now();
            now.trouble = Some(Trouble { at: now.since, text });
        }
    })
}
