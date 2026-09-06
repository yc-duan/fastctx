//! Key-free local observation, including recovery data when the node cannot answer.

use super::{Action, Failure, ReadState, Snapshot};
use crate::control::paths::ControlPaths;
use crate::world::node::admin::{self, AdminRequest};
use crate::world::{WorldPaths, load_config};
use std::path::PathBuf;
use std::time::Duration;

pub(super) async fn read(paths: &ControlPaths, hub_data: PathBuf) -> Snapshot {
    let world = WorldPaths::from_control(paths);
    let config = ReadState::optional(load_config(&world));
    let enrolled = matches!(config, ReadState::Ready(_));
    let node = if enrolled {
        match ask(&AdminRequest::Snapshot, Duration::from_secs(3)).await {
            Ok(data) => ReadState::result(
                serde_json::from_value(data)
                    .map_err(|error| format!("Cannot decode the node snapshot: {error}")),
            ),
            Err(error) => ReadState::Failed(error),
        }
    } else {
        ReadState::Missing
    };
    Snapshot {
        observed_at: crate::world::now_rfc3339(),
        last_status: if enrolled {
            ReadState::optional(crate::world::node::read_status(&world))
        } else {
            ReadState::Missing
        },
        description: if enrolled {
            ReadState::result(crate::world::notes::description(&world))
        } else {
            ReadState::Missing
        },
        service_installed: ReadState::result(crate::world::node::service::inspect(paths)),
        hub: ReadState::optional(crate::world::hub::read_status(&hub_data)),
        hub_data,
        config,
        node,
    }
}

/// Bounds connection, request writing, and reply reading together. An unavailable endpoint
/// does not authorize a second hub connection from the helper.
pub(super) async fn ask(
    request: &AdminRequest,
    timeout: Duration,
) -> Result<serde_json::Value, Failure> {
    let environment = crate::session::SessionEnvironment::capture().map_err(Failure::from)?;
    let endpoint = crate::runtime::node_admin_endpoint(&environment).map_err(Failure::from)?;
    let response = tokio::time::timeout(timeout, admin::call(&endpoint, request, timeout)).await
        .map_err(|_| Failure::new(format!("The node service did not answer within {} seconds.", timeout.as_secs())))?
        .map_err(Failure::from)?
        .ok_or_else(|| Failure::new("node_service_not_running: No node service answers for this user and this build. Start the node service, or restart it after upgrading FastCtx."))?;
    if response.ok {
        Ok(response.data)
    } else {
        Err(Failure::new(response.error.unwrap_or_else(|| {
            "The node rejected the operation without an explanation.".to_string()
        })))
    }
}

pub(super) fn mutation_failure(mut failure: Failure, action: &Action) -> Failure {
    if !action.is_read()
        && matches!(
            failure.kind,
            super::FailureKind::Timeout | super::FailureKind::Other
        )
    {
        failure.uncertain = true;
    }
    failure
}
