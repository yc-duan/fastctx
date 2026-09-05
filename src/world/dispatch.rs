//! One entry for every World call, whichever door it came through: the MCP tool surface
//! (`surface.rs`) or the command line behind the node's admin channel (`node/admin.rs`).
//!
//! A call names one machine. This machine's own name runs the verb here, with the caller's
//! session and without consulting the member table, so a freshly enrolled machine that has
//! not reached the hub yet is still a legal target; any other name goes through the hub.

use super::client::{NodeOutcome, WorldClient};
use super::messages::{Call, CallBudget, CallOrigin};
use super::node::executor;
use crate::session::SessionContext;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// One verb on one machine.
pub(crate) struct NodeCall {
    pub(crate) verb: String,
    pub(crate) node: String,
    /// The tool's own request object, without the `node` selector.
    pub(crate) args: serde_json::Value,
    pub(crate) budget: CallBudget,
    pub(crate) timeout: Duration,
    pub(crate) origin: CallOrigin,
}

/// Runs the call and renders its outcome; a machine that is offline, forbidden, or silent is an
/// outcome with a status, not an error.
pub(crate) async fn call_node(
    client: &Arc<WorldClient>,
    session: Arc<SessionContext>,
    call: NodeCall,
) -> Result<NodeOutcome, String> {
    if call.node == client.name() {
        return Ok(run_on_this_machine(client, session, call).await);
    }
    client.resolve_target(&call.node)?;
    client
        .call(
            &call.verb,
            &call.node,
            call.args,
            call.budget,
            call.timeout,
            call.origin,
        )
        .await
}

/// Executes a World call whose target is this very machine, with the caller's own session.
async fn run_on_this_machine(
    client: &Arc<WorldClient>,
    session: Arc<SessionContext>,
    call: NodeCall,
) -> NodeOutcome {
    let wire = Call {
        verb: call.verb,
        args: call.args,
        budget: CallBudget::default(),
        cwd: None,
        timeout_ms: call.timeout.as_millis() as u64,
        grant_revision: 0,
        origin: call.origin,
    };
    let outcome = executor::execute(client, wire, session, CancellationToken::new()).await;
    NodeOutcome {
        node: client.name(),
        status: if outcome.response.is_error {
            "error"
        } else {
            "ok"
        }
        .to_string(),
        response: Some(outcome.response),
        message: None,
        facts: outcome.facts,
    }
}
