//! What the World keeps on this machine, and deleting it: the requests the
//! settings page 本机数据, the TUI and `fastctx data` make (`DataCall`). They
//! are answered here and never go to another machine: each machine manages
//! only its own data.

pub(crate) mod delete;
mod host;
pub(crate) mod places;
mod space;

use std::sync::Arc;

use fastctx_types::data::{DataCall, DataReply};
use fastctx_types::ipc::Failure;

use crate::Node;

pub(crate) async fn answer(node: &Arc<Node>, call: DataCall) -> Result<DataReply, Failure> {
    match call {
        DataCall::List => {
            let node = node.clone();
            let space = fastctx_platform::task::spawn_blocking(move || space::measure(&node))
                .await
                .map_err(|error| Failure::new(error.to_string()))?
                .map_err(|error| Failure::new(format!("measuring what is kept here: {error}")))?;
            Ok(DataReply::Listed(Box::new(space)))
        }
        DataCall::Delete { names } => Ok(DataReply::Deleted { deletions: delete::delete(node, names).await }),
        DataCall::ClearCache => {
            let root = node.root.join("cache");
            let bytes = fastctx_platform::task::spawn_blocking(move || -> std::io::Result<u64> {
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
            if days.is_some() {
                return Err(Failure::not_run("Job output is an asset and has no retention deadline."));
            }
            Ok(DataReply::OutputDays { days: 30, output_retention: Some("unlimited".into()) })
        }
    }
}
