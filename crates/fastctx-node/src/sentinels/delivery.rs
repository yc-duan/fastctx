//! Stable sentinel events enter the same durable inbox and letters as other answers.
use crate::Node;
use fastctx_sentinel::{
    lifecycle,
    resident::{Event, Host, Notification},
};
use fastctx_types::{
    SentinelId,
    access::{Acting, HubAccount, Written},
    residents::Notice,
    sentinel_notice::{Event as NoticeEvent, SentinelNotice},
};
use std::sync::Arc;

pub(super) async fn pending(node: &Arc<Node>, host: &Host, scope: &HubAccount, id: &SentinelId) -> Result<(), String> {
    for notification in host.notifications(scope, id).await? {
        // Withdrawal or reassignment wins over an observation waiting to be delivered.
        let Some((_, current)) = lifecycle::get(host.records.as_ref(), scope, id).await.map_err(|e| e.to_string())?
        else {
            return Ok(());
        };
        if current.definition != notification.registration.definition {
            return Ok(());
        }
        send(node, &notification).await?;
        host.delivered(&notification).await?;
    }
    // A finished sentinel's definition goes once all it found has reached its
    // owner, and only its home knows that.
    let Some((version, current)) = lifecycle::get(host.records.as_ref(), scope, id).await.map_err(|e| e.to_string())?
    else {
        return Ok(());
    };
    if !current.active && node.is_me(&current.definition.home) && host.notifications(scope, id).await?.is_empty() {
        tokio::time::timeout(
            std::time::Duration::from_secs(60),
            host.records.compare_exchange(scope, lifecycle::KIND, id.as_str(), Some(version), None),
        )
        .await
        .map_err(|_| "removing a finished sentinel timed out")?
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}
async fn send(node: &Arc<Node>, notification: &Notification) -> Result<(), String> {
    let registration = &notification.registration;
    let definition = &registration.definition;
    let scope = &registration.scope;
    let link = node.link_of(&scope.hub).ok_or("the sentinel account's hub is not connected")?;
    let destination = Written { name: definition.owner.machine.clone(), hub: Some(link.hub_name()) };
    let event = match &notification.event {
        Event::Alarm { index } => NoticeEvent::Alarm { index: *index },
        Event::Satisfied { reason, evidence } => {
            NoticeEvent::Satisfied { reason: reason.clone(), evidence: evidence.clone() }
        }
        Event::Unable { reason } => NoticeEvent::Unable { reason: reason.clone() },
    };
    let notice = Notice::Sentinel {
        observation: Box::new(SentinelNotice {
            sentinel: definition.id.clone(),
            generation: definition.generation,
            machine: definition.home.clone(),
            at: jiff::Timestamp::from_millisecond(notification.at).map_err(|e| e.to_string())?,
            due: jiff::Timestamp::from_millisecond(notification.due).map_err(|e| e.to_string())?,
            condition: definition.condition.clone(),
            tell: definition.tell.clone(),
            event,
        }),
    };
    let identity = crate::speech::fixed_id("sentinel", &[&notification.id]);
    let acting = Acting::one(scope.hub.clone(), scope.account.clone());
    let result = crate::speech::tell_at(
        node,
        &acting,
        &definition.owner,
        &destination,
        identity,
        notice,
        crate::speech::Owed::Nobody,
    )
    .await;
    match result {
        Err(error) if crate::lifecycle::refused::reported(&error) => {
            super::report(node, &format!("delivery {}", definition.owner.resident), Some(error.message));
            Ok(())
        }
        Err(error) => Err(error.message),
        Ok(_) => Ok(()),
    }
}
