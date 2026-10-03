//! Local IPC contract for resident processes. JSON operations share the node's identity.
use super::{
    client::WorldClient,
    ledger::{LedgerFile, ProjectUpdate},
    node::admin::CallOutcome,
};
use serde::{Deserialize, Serialize};
use std::{sync::Arc, time::Duration};

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub(crate) enum Request {
    SendMail {
        to: String,
        body: serde_json::Value,
    },
    Inbox,
    WaitInbox,
    DeliveryFailures,
    AcknowledgeMail {
        receipt: String,
    },
    AppendLedger {
        scope: String,
        files: Vec<LedgerFile>,
        project: Option<ProjectUpdate>,
    },
    ReadLedger {
        scope: String,
    },
    SyncLedger,
    Subscribe {
        node: String,
        topic: serde_json::Value,
    },
    SubscriptionNext,
    PublishEvent {
        id: String,
        value: serde_json::Value,
    },
    Unsubscribe {
        id: String,
    },
    CallMany {
        nodes: Vec<String>,
        verb: String,
        args: serde_json::Value,
        timeout_ms: u64,
    },
}
pub(crate) async fn handle(
    client: &Arc<WorldClient>,
    request: Request,
) -> Result<serde_json::Value, String> {
    match request {
        Request::SendMail { to, body } => {
            Ok(serde_json::json!({"id": client.send_mail(&to, body)?}))
        }
        Request::DeliveryFailures => value(super::delivery::failures(&client.paths)?),
        Request::Inbox => value(client.inbox()?),
        Request::WaitInbox => value(client.wait_inbox().await?),
        Request::AcknowledgeMail { receipt } => {
            client.acknowledge_mail(&receipt)?;
            Ok(serde_json::json!({"acknowledged": receipt}))
        }
        Request::AppendLedger {
            scope,
            files,
            project,
        } => value(client.append_ledger(scope, files, project).await?),
        Request::ReadLedger { scope } => value(client.read_ledger(&scope)?),
        Request::SyncLedger => {
            client.sync_ledger().await?;
            Ok(serde_json::Value::Null)
        }
        Request::Subscribe { node, topic } => {
            Ok(serde_json::json!({"id": client.subscribe(&node, topic).await?}))
        }
        Request::SubscriptionNext => value(client.subscriptions.next().await),
        Request::PublishEvent { id, value } => {
            client.publish_event(&id, value)?;
            Ok(serde_json::Value::Null)
        }
        Request::Unsubscribe { id } => {
            client.unsubscribe(&id)?;
            Ok(serde_json::Value::Null)
        }
        Request::CallMany {
            nodes,
            verb,
            args,
            timeout_ms,
        } => {
            let outcomes = client
                .call_many(nodes, &verb, args, Duration::from_millis(timeout_ms))
                .await;
            value(
                outcomes
                    .into_iter()
                    .map(|outcome| CallOutcome {
                        node: outcome.node,
                        status: outcome.status,
                        message: outcome.message,
                        response: outcome
                            .response
                            .as_ref()
                            .map(super::messages::WireResponse::from),
                        facts: outcome.facts,
                    })
                    .collect::<Vec<_>>(),
            )
        }
    }
}
fn value(value: impl Serialize) -> Result<serde_json::Value, String> {
    serde_json::to_value(value).map_err(|e| e.to_string())
}
