//! Goal lifecycle and the deliberately small material supplied for a judgment.
use crate::{
    lifecycle::{self, Homes},
    records::{Error, Records},
};
use fastctx_types::{access::HubAccount, sentinel::*, sentinel_control::Registration};
use serde_json::{Value, json};

/// Produces only the columns belonging to this trigger. Oversized material is
/// refused explicitly: the user's goal and evidence paths are never truncated.
pub fn input(review: &GoalReview) -> Result<Value, String> {
    if review.goal.trim().is_empty() || review.goal.len() > 8192 || !(1..=2).contains(&review.suggestions_left) {
        return Err("invalid goal or remaining suggestion count".into());
    }
    let mut value = json!({"trigger":review.trigger,"suggestions_left":review.suggestions_left,
        "goal":review.goal,"previous_judgments":review.previous_judgments});
    if review.trigger == GoalTrigger::Paused {
        value["user_words"] = json!(review.user_words);
    } else {
        value["plan_claim"] = json!(review.plan_claim);
        value["actions"] = json!(review.actions);
        value["machines"] = json!(review.machines);
        value["now"] = json!(review.now);
        if review.trigger == GoalTrigger::Idle {
            value["turn_end"] = json!(review.turn_end);
            value["last_words_claim"] = json!(review.last_words_claim);
        } else {
            value["blocked_claim"] = json!(review.blocked_claim);
        }
    }
    if let Some(full) = &review.full {
        value["full"] = json!(full);
    }
    if serde_json::to_vec(&value).map_err(|e| e.to_string())?.len() > 24_000 {
        return Err(
            "goal review exceeds 24000 bytes; supply a smaller account of the facts and keep full evidence paths"
                .into(),
        );
    }
    Ok(value)
}

/// Opens or revises the goal's deterministic sentinel ID. A newly assigned
/// generation must never be reused, even after deletion and recreation.
pub async fn open(
    records: &dyn Records,
    homes: &dyn Homes,
    mut registration: Registration,
) -> Result<(Registration, Option<String>), Error> {
    let fail = |s: &str| Error::Unavailable(s.into());
    if registration.definition.kind != Kind::Goal
        || registration.definition.condition.trim().is_empty()
        || registration.definition.condition.len() > 8192
        || !registration.definition.at.is_empty()
    {
        return Err(fail("invalid goal sentinel definition"));
    }
    let current = lifecycle::get(records, &registration.scope, &registration.definition.id).await?;
    let expected = current.as_ref().map(|(version, _)| *version);
    if let Some((version, old)) = current {
        if old.definition.kind != Kind::Goal || old.definition.owner != registration.definition.owner {
            return Err(fail("goal sentinel identity is already assigned"));
        }
        if old.active
            && old.definition.condition == registration.definition.condition
            && old.definition.home == registration.definition.home
        {
            let reason = begin(homes, &old, version).await;
            return Ok((old, reason));
        }
        if old.definition.generation == registration.definition.generation {
            return Err(fail("a revised goal needs a fresh sentinel generation"));
        }
    }
    registration.active = true;
    let value = serde_json::to_value(&registration).map_err(|e| Error::Unavailable(e.to_string()))?;
    let record = tokio::time::timeout(
        std::time::Duration::from_secs(60),
        records.compare_exchange(
            &registration.scope,
            lifecycle::KIND,
            registration.definition.id.as_str(),
            expected,
            Some(value),
        ),
    )
    .await
    .map_err(|_| fail("opening the goal sentinel timed out; outcome unknown"))??
    .ok_or_else(|| fail("goal sentinel registration was not returned"))?;
    let reason = begin(homes, &registration, record.version).await;
    Ok((registration, reason))
}
async fn begin(homes: &dyn Homes, registration: &Registration, version: u64) -> Option<String> {
    match tokio::time::timeout(std::time::Duration::from_secs(30), homes.begin(registration, version)).await {
        Ok(Ok(_)) => None,
        Ok(Err(error)) => Some(error),
        Err(_) => Some("the goal sentinel has not answered yet".into()),
    }
}
/// Ends exactly the assignment the engine retained. A stale end cannot remove a
/// newly opened goal. Durable deletion does not depend on the home being online.
pub async fn end(
    records: &dyn Records,
    homes: &dyn Homes,
    registration: &Registration,
) -> Result<Option<String>, Error> {
    let Some((version, current)) = lifecycle::get(records, &registration.scope, &registration.definition.id).await?
    else {
        return Ok(None);
    };
    if current != *registration || current.definition.kind != Kind::Goal {
        return Err(Error::Unavailable("goal sentinel assignment changed".into()));
    }
    tokio::time::timeout(
        std::time::Duration::from_secs(60),
        records.compare_exchange(
            &registration.scope,
            lifecycle::KIND,
            registration.definition.id.as_str(),
            Some(version),
            None,
        ),
    )
    .await
    .map_err(|_| Error::Unavailable("ending the goal sentinel timed out; outcome unknown".into()))??;
    Ok(match tokio::time::timeout(std::time::Duration::from_secs(15), homes.withdraw(registration)).await {
        Ok(Ok(())) => None,
        Ok(Err(error)) => Some(error),
        Err(_) => Some("goal sentinel home has not confirmed withdrawal".into()),
    })
}
/// Ordinary sentinels can wake their owner. The goal's own judge never counts as
/// background work; including it would suppress every suspicious-stop review.
pub async fn waiting(records: &dyn Records, scope: &HubAccount) -> Result<Vec<Registration>, Error> {
    tokio::time::timeout(std::time::Duration::from_secs(60), async {
        let mut result = Vec::new();
        for head in records.list(scope, lifecycle::KIND).await? {
            let id = head.key.parse().map_err(|e: fastctx_types::NameError| Error::Unavailable(e.to_string()))?;
            if let Some((_, registration)) = lifecycle::get(records, scope, &id).await?
                && registration.active
                && registration.definition.kind != Kind::Goal
            {
                result.push(registration);
            }
        }
        Ok(result)
    })
    .await
    .map_err(|_| Error::Unavailable("reading sentinel waits timed out".into()))?
}
