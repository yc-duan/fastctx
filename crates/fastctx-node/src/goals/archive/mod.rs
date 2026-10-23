//! Immutable account history and text, referenced by the goal's accepted CAS revision.

mod codec;
mod collect;
pub(crate) use codec::{Storage, decode};
pub(crate) use collect::{for_resident, start};

use super::{
    operations,
    records::{Exchanged, Record},
};
use crate::Node;
use fastctx_types::ResidentId;
use fastctx_types::access::HubAccount;
use fastctx_types::goals::{Decision, Goal, HistoryPage, HistoryRef};
use fastctx_types::ipc::Failure;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

const TEXT: &str = "goal_text";
const HISTORY: &str = "goal_history";
const WRITES: &str = "goal_writes";
const INLINE_HISTORY: usize = 16 * 1024;
const PAGE_BYTES: usize = 24 * 1024;
const RECORD_BYTES: usize = 64 * 1024;

#[derive(Clone, Serialize, Deserialize)]
struct Scope {
    goal: String,
    resident: ResidentId,
    reservation: String,
}

#[derive(Clone, Serialize, Deserialize)]
struct Reservation {
    goal: String,
    resident: ResidentId,
    expected: Option<u64>,
}

pub(crate) struct Writer<'a> {
    node: &'a Arc<Node>,
    account: &'a HubAccount,
    scope: Scope,
    expected: Option<u64>,
    reserved: bool,
    established: bool,
    texts: std::collections::BTreeMap<String, codec::TextRef>,
}

fn failure(error: impl std::fmt::Display) -> Failure {
    Failure::new(error.to_string())
}
fn key(prefix: &str, bytes: &[u8]) -> String {
    format!("{prefix}-{}", &fastctx_hands::files::content_hash(bytes)[..48])
}
fn reservation_key(resident: &ResidentId, operation: &str) -> String {
    key("w", &serde_json::to_vec(&(resident, operation)).expect("identities serialize"))
}

async fn read(node: &Node, account: &HubAccount, kind: &str, key: &str) -> Result<Option<Record>, Failure> {
    operations::bounded(node.goals.records.read(account, kind, key)).await
}

async fn immutable(node: &Node, account: &HubAccount, kind: &str, key: &str, body: Value) -> Result<(), Failure> {
    if serde_json::to_vec(&body).map_err(failure)?.len() > RECORD_BYTES {
        return Err(Failure::new("the encoded goal archive record exceeds 64 KiB"));
    }
    match operations::bounded(node.goals.records.exchange(account, kind, key, None, Some(body.clone()))).await? {
        Exchanged::Written { .. } => Ok(()),
        Exchanged::Conflict { .. } => {
            if read(node, account, kind, key).await?.and_then(|record| record.body).as_ref() == Some(&body) {
                Ok(())
            } else {
                Err(Failure::new(format!("immutable goal archive {kind}/{key} differs from the proposed content")))
            }
        }
    }
}

impl Writer<'_> {
    async fn reserve(&mut self) -> Result<(), Failure> {
        if !self.reserved && !self.established {
            let body = serde_json::to_value(Reservation {
                goal: self.scope.goal.clone(),
                resident: self.scope.resident.clone(),
                expected: self.expected,
            })
            .map_err(failure)?;
            immutable(self.node, self.account, WRITES, &self.scope.reservation, body).await?;
            self.reserved = true;
        }
        Ok(())
    }
}

pub(crate) async fn encode(
    node: &Arc<Node>,
    account: &HubAccount,
    goal: &Goal,
    operation: &str,
    expected: Option<u64>,
    previous: Storage,
) -> Result<Value, Failure> {
    let mut writer = Writer {
        node,
        account,
        scope: Scope {
            goal: goal.id.clone(),
            resident: goal.resident.resident.clone(),
            reservation: reservation_key(&goal.resident.resident, operation),
        },
        expected,
        reserved: false,
        established: previous.owner.as_deref() == Some(&goal.id),
        texts: previous.texts,
    };
    let mut goal = goal.clone();
    if serde_json::to_vec(&goal.decisions).map_err(failure)?.len() > INLINE_HISTORY && goal.decisions.len() > 1 {
        let mut keep = goal.decisions.len() - 1;
        while keep > 0 && serde_json::to_vec(&goal.decisions[keep - 1..]).map_err(failure)?.len() <= INLINE_HISTORY {
            keep -= 1;
        }
        let older: Vec<_> = goal.decisions.drain(..keep).collect();
        let mut batch = Vec::new();
        let mut bytes = 0;
        for decision in older {
            let size = serde_json::to_vec(&decision).map_err(failure)?.len();
            if !batch.is_empty() && bytes + size > PAGE_BYTES {
                goal.history = Some(page(&mut writer, goal.history.take(), std::mem::take(&mut batch)).await?);
                bytes = 0;
            }
            bytes += size;
            batch.push(decision);
        }
        if !batch.is_empty() {
            goal.history = Some(page(&mut writer, goal.history.take(), batch).await?);
        }
    }
    let body = codec::encode(&mut writer, serde_json::to_value(goal).map_err(failure)?).await?;
    if serde_json::to_vec(&body).map_err(failure)?.len() > RECORD_BYTES {
        return Err(Failure::new(
            "the goal's structured record exceeds 64 KiB after archiving its history and long text",
        ));
    }
    Ok(body)
}

async fn page(
    writer: &mut Writer<'_>,
    previous: Option<HistoryRef>,
    decisions: Vec<Decision>,
) -> Result<HistoryRef, Failure> {
    writer.reserve().await?;
    let count = previous
        .as_ref()
        .map_or(0, |previous| previous.count)
        .checked_add(decisions.len() as u64)
        .ok_or_else(|| Failure::new("goal history count overflow"))?;
    let mut value = serde_json::to_value(HistoryPage {
        goal: writer.scope.goal.clone(),
        resident: writer.scope.resident.clone(),
        previous,
        decisions,
    })
    .map_err(failure)?;
    value["reservation"] = Value::String(writer.scope.reservation.clone());
    let value = codec::encode(writer, value).await?;
    let key = key("h", &serde_json::to_vec(&value).map_err(failure)?);
    immutable(writer.node, writer.account, HISTORY, &key, value).await?;
    Ok(HistoryRef { key, count })
}

pub(crate) async fn page_at(node: &Arc<Node>, goal: &Goal, reference: &HistoryRef) -> Result<HistoryPage, Failure> {
    let body = read(node, &goal.account, HISTORY, &reference.key)
        .await?
        .and_then(|record| record.body)
        .ok_or_else(|| Failure::new(format!("goal history page {} is missing", reference.key)))?;
    if key("h", &serde_json::to_vec(&body).map_err(failure)?) != reference.key {
        return Err(Failure::new("goal history content does not match its reference"));
    }
    let (body, _) = decode(node, &goal.account, body).await?;
    let page: HistoryPage = serde_json::from_value(body).map_err(failure)?;
    if page.decisions.is_empty()
        || page.goal != goal.id
        || page.resident != goal.resident.resident
        || page.previous.as_ref().map_or(0, |previous| previous.count).checked_add(page.decisions.len() as u64)
            != Some(reference.count)
    {
        return Err(Failure::new("goal history ownership or count does not match its reference"));
    }
    Ok(page)
}

pub(crate) async fn contains(node: &Arc<Node>, goal: &Goal, operation: &str) -> Result<bool, Failure> {
    let mut next = goal.history.clone();
    while let Some(reference) = next {
        let page = page_at(node, goal, &reference).await?;
        if page.decisions.iter().any(|decision| decision.operation == operation) {
            return Ok(true);
        }
        next = page.previous;
    }
    Ok(false)
}

pub(crate) async fn history(
    node: &Arc<Node>,
    goal: &Goal,
    requested: Option<&str>,
) -> Result<Option<HistoryPage>, Failure> {
    let mut next = goal.history.clone();
    while let Some(reference) = next {
        let page = page_at(node, goal, &reference).await?;
        if requested.is_none_or(|requested| requested == reference.key) {
            return Ok(Some(page));
        }
        next = page.previous;
    }
    if requested.is_some() {
        Err(Failure::new("this page is not part of the goal's accepted history"))
    } else {
        Ok(None)
    }
}

pub(crate) async fn close(
    node: &Node,
    account: &HubAccount,
    resident: &ResidentId,
    operation: &str,
) -> Result<(), Failure> {
    let key = reservation_key(resident, operation);
    if let Some(record) = read(node, account, WRITES, &key).await?
        && record.body.is_some()
    {
        match operations::bounded(node.goals.records.exchange(account, WRITES, &key, Some(record.version), None))
            .await?
        {
            Exchanged::Written { .. } => {}
            Exchanged::Conflict { .. } => {
                if read(node, account, WRITES, &key).await?.is_some_and(|record| record.body.is_some()) {
                    return Err(Failure::new("the goal write reservation changed while closing it"));
                }
            }
        }
    }
    Ok(())
}

pub(crate) async fn decisions(node: &Arc<Node>, goal: &Goal) -> Result<Vec<Decision>, Failure> {
    let mut pages = Vec::new();
    let mut next = goal.history.clone();
    while let Some(reference) = next {
        let page = page_at(node, goal, &reference).await?;
        next = page.previous;
        pages.push(page.decisions);
    }
    let mut decisions: Vec<_> = pages.into_iter().rev().flatten().collect();
    decisions.extend(goal.decisions.clone());
    Ok(decisions)
}
