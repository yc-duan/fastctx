//! History through a local node or a device's direct hub connection.

use crate::{CallError, Client, Error, World};
use fastctx_types::{
    Call, Reply,
    history::{Catalog, HistoryCall, HistoryReply, Scope},
    ipc::NodeFrame,
};
use std::sync::Arc;

impl World {
    pub async fn history(&self, request: HistoryCall) -> Result<HistoryReply, Error> {
        enabled()?;
        match self.ask(Call::History(request)).await? {
            Reply::History(reply) => Ok(reply),
            other => Err(Error::Unexpected(format!("{other:?}"))),
        }
    }

    pub async fn watch_history(&self, scope: Scope) -> Result<HistoryWatch, Error> {
        enabled()?;
        if let Some(device) = self.device() {
            return Ok(HistoryWatch::Device {
                device: device.clone(),
                scope,
                changes: device.world_changes(),
                first: true,
                told: None,
            });
        }
        let (client, id) = self.connect().await?.watch_history(scope).await?;
        Ok(HistoryWatch::Local { client, id })
    }
}

pub enum HistoryWatch {
    Local {
        client: Client,
        id: u64,
    },
    Device {
        device: Arc<crate::device::Device>,
        scope: Scope,
        changes: tokio::sync::watch::Receiver<u64>,
        first: bool,
        told: Option<Catalog>,
    },
}

impl HistoryWatch {
    pub async fn next(&mut self) -> Result<Catalog, Error> {
        match self {
            Self::Local { client, id } => match client.frame().await? {
                NodeFrame::Reply { id: reply_id, reply } if reply_id == *id => match reply {
                    Ok(Reply::History(HistoryReply::Catalog { catalog })) => Ok(catalog),
                    Err(error) => Err(CallError::Failed(error).into()),
                    other => Err(Error::Unexpected(format!("{other:?}"))),
                },
                other => Err(Error::Unexpected(format!("{other:?}"))),
            },
            Self::Device { device, scope, changes, first, told } => loop {
                if !std::mem::take(first) {
                    changes
                        .changed()
                        .await
                        .map_err(|_| CallError::Broken(std::io::Error::other("the device closed")))?;
                }
                changes.borrow_and_update();
                let (reply, _) = device.call(Call::History(HistoryCall::List { scope: scope.clone() })).await?;
                let Reply::History(HistoryReply::Catalog { catalog }) = reply else {
                    return Err(Error::Unexpected(format!("{reply:?}")));
                };
                if told.as_ref() != Some(&catalog) {
                    *told = Some(catalog.clone());
                    return Ok(catalog);
                }
            },
        }
    }
}

pub(crate) fn enabled() -> Result<(), Error> {
    if fastctx_platform::history() {
        Ok(())
    } else {
        Err(CallError::Failed(fastctx_types::ipc::Failure::not_run("history is not enabled in this release")).into())
    }
}
