//! What the World keeps on this machine, and deleting it: the requests the
//! settings page 本机数据, the TUI and `fastctx data` make (`DataCall`). They
//! are answered here and never go to another machine: each machine manages
//! only its own data.

pub(crate) mod delete;
mod host;
pub(crate) mod places;
mod space;

use std::sync::Arc;

use fastctx_types::data::{DataCall, DataReply, OUTPUT_DAYS};
use fastctx_types::ipc::Failure;

use crate::Node;

pub(crate) async fn answer(node: &Arc<Node>, call: DataCall) -> Result<DataReply, Failure> {
    match call {
        DataCall::List => {
            let node = node.clone();
            let space = tokio::task::spawn_blocking(move || space::measure(&node))
                .await
                .map_err(|error| Failure::new(error.to_string()))?
                .map_err(|error| Failure::new(format!("measuring what is kept here: {error}")))?;
            Ok(DataReply::Listed(Box::new(space)))
        }
        DataCall::Delete { names } => Ok(DataReply::Deleted { deletions: delete::delete(node, names).await }),
        DataCall::ClearCache => {
            let root = node.root.join("cache");
            let bytes = tokio::task::spawn_blocking(move || -> std::io::Result<u64> {
                let viewed = root.join("viewed");
                let mut turn = fastctx_platform::viewed_cache::Turn::acquire(&viewed)?;
                let before = space::tally(&root)?.bytes;
                turn.clear()?;
                for path in space::entries(&root)? {
                    if path != viewed {
                        fastctx_platform::owned_directory::delete(&path)?;
                    }
                }
                Ok(before.saturating_sub(space::tally(&root)?.bytes))
            })
            .await
            .map_err(|error| Failure::new(error.to_string()))?
            .map_err(|error| Failure::new(format!("clearing this machine's cache: {error}")))?;
            Ok(DataReply::CacheCleared { bytes })
        }
        DataCall::OutputDays { days } => {
            if let Some(days) = days {
                if !OUTPUT_DAYS.contains(&days) {
                    return Err(Failure::not_run(format!(
                        "command output can be kept {} days",
                        OUTPUT_DAYS.map(|days| days.to_string()).join(", ")
                    )));
                }
                crate::retention::set_days(&node.world, days)
                    .map_err(|error| Failure::new(format!("keeping the setting: {error}")))?;
            }
            let days = crate::retention::days(&node.world)
                .map_err(|error| Failure::new(format!("reading the setting: {error}")))?;
            Ok(DataReply::OutputDays { days })
        }
    }
}
