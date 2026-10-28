//! A chunk is collectible only when no accepted goal or still-possible CAS can reference it.

use super::{HISTORY, Reservation, Scope, TEXT, WRITES, failure, read};
use crate::Node;
use fastctx_types::ResidentId;
use fastctx_types::access::HubAccount;
use fastctx_types::ipc::Failure;
use std::sync::Arc;

async fn discard(node: &Node, account: &HubAccount, kind: &str, key: &str, version: u64) -> Result<(), Failure> {
    match super::operations::bounded(node.goals.records.exchange(account, kind, key, Some(version), None)).await? {
        super::Exchanged::Written { .. } => Ok(()),
        super::Exchanged::Conflict { .. } => {
            if read(node, account, kind, key).await?.is_none_or(|record| record.body.is_none()) {
                Ok(())
            } else {
                Err(Failure::new("the goal archive changed during collection"))
            }
        }
    }
}

async fn collect(
    node: &Arc<Node>,
    account: &HubAccount,
    kind: &str,
    key: &str,
    only: Option<&ResidentId>,
) -> Result<(), Failure> {
    let Some(record) = read(node, account, kind, key).await? else {
        return Ok(());
    };
    let Some(body) = record.body else {
        return Ok(());
    };
    let (goal, resident, reservation) = if kind == WRITES {
        let reservation: Reservation = serde_json::from_value(body).map_err(failure)?;
        (reservation.goal.clone(), reservation.resident.clone(), Some(reservation))
    } else {
        let scope: Scope = serde_json::from_value(body).map_err(failure)?;
        if only.is_some_and(|only| only != &scope.resident) {
            return Ok(());
        }
        let reservation = read(node, account, WRITES, &scope.reservation)
            .await?
            .and_then(|record| record.body)
            .map(serde_json::from_value::<Reservation>)
            .transpose()
            .map_err(failure)?;
        if reservation
            .as_ref()
            .is_some_and(|reservation| reservation.goal != scope.goal || reservation.resident != scope.resident)
        {
            return Err(Failure::new("a goal chunk's reservation belongs to another goal"));
        }
        (scope.goal, scope.resident, reservation)
    };
    if only.is_some_and(|only| only != &resident) {
        return Ok(());
    }
    let parent = read(node, account, super::operations::KIND, resident.as_str()).await?;
    let version = parent.as_ref().map(|record| record.version);
    let current_goal = parent
        .as_ref()
        .and_then(|record| record.body.as_ref())
        .map(|body| body["id"].as_str().ok_or_else(|| Failure::new("the parent goal has no identity")))
        .transpose()?;
    let accepted = current_goal == Some(goal.as_str());
    let possible = reservation.as_ref().is_some_and(|reservation| reservation.expected == version);
    let obsolete = if kind == WRITES { accepted || !possible } else { !accepted && !possible };
    if obsolete {
        discard(node, account, kind, key, record.version).await?;
    }
    Ok(())
}

pub(crate) async fn for_resident(node: &Arc<Node>, account: &HubAccount, resident: &ResidentId) -> Result<(), Failure> {
    for kind in [TEXT, HISTORY, WRITES] {
        for row in super::operations::bounded(node.goals.records.list(account, kind)).await? {
            collect(node, account, kind, &row.key, Some(resident)).await?;
        }
    }
    Ok(())
}

pub(crate) fn start(node: &Arc<Node>, account: &HubAccount) {
    for kind in [TEXT, HISTORY, WRITES] {
        let (node, account) = (node.clone(), account.clone());
        tokio::spawn(async move {
            let mut again = crate::again::Again::default();
            let mut previous = None;
            loop {
                if let Err(error) = follow(&node, &account, kind).await
                    && previous.as_ref() != Some(&error.message)
                {
                    node.log.line(format_args!("collecting account goal archives: {}", error.message));
                    previous = Some(error.message);
                }
                tokio::time::sleep(again.next()).await;
            }
        });
    }
}

async fn follow(node: &Arc<Node>, account: &HubAccount, kind: &str) -> Result<(), Failure> {
    let mut changes = super::operations::bounded(node.goals.records.watch(account, kind)).await?;
    for row in super::operations::bounded(node.goals.records.list(account, kind)).await? {
        collect(node, account, kind, &row.key, None).await?;
    }
    loop {
        for row in changes.next().await? {
            collect(node, account, kind, &row.key, None).await?;
        }
    }
}
