//! Network operations other processes on this machine ask the node for over its admin channel,
//! under the node's identity. Mail and subscriptions are not among them: the node's town is
//! their one consumer (`town::remote`), so no other reader can take a letter or an event
//! meant for it.
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
    DeliveryFailures,
    AppendLedger {
        scope: String,
        files: Vec<LedgerFile>,
        project: Option<ProjectUpdate>,
    },
    ReadLedger {
        scope: String,
    },
    SyncLedger,
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
        Request::DeliveryFailures => value(super::delivery::failures(&client.paths)?),
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
