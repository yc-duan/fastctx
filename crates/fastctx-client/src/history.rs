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
            let (link, account) = device.history_network(&scope).await?;
            let changes = link
                .history_catalog_changes(&account, &scope)
                .map_err(|error| CallError::Failed(fastctx_types::ipc::Failure::new(error)))?;
            return Ok(HistoryWatch::Device {
                device: device.clone(),
                scope: scope.clone(),
                changes,
                status: link.subscribe(),
                world_changes: device.world_changes(),
                readers: device.history_readers(&scope),
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
        status: tokio::sync::watch::Receiver<fastctx_net::link::Status>,
        world_changes: tokio::sync::watch::Receiver<u64>,
        readers: Vec<(fastctx_types::access::HubId, Vec<fastctx_types::AccountName>)>,
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
            Self::Device { device, scope, changes, status, world_changes, readers, first, told } => loop {
                if !std::mem::take(first) {
                    tokio::select! {
                        changed = changes.changed() => changed,
                        changed = status.changed() => changed,
                        changed = async {
                            loop {
                                world_changes.changed().await?;
                                if device.history_readers(scope) != *readers { return Ok(()); }
                            }
                        } => changed,
                    }
                    .map_err(|_| CallError::Broken(std::io::Error::other("the device closed")))?;
                }
                world_changes.borrow_and_update();
                *readers = device.history_readers(scope);
                let (link, account) = device.history_network(scope).await?;
                *changes = link
                    .history_catalog_changes(&account, scope)
                    .map_err(|error| CallError::Failed(fastctx_types::ipc::Failure::new(error)))?;
                *status = link.subscribe();
                changes.borrow_and_update();
                status.borrow_and_update();
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
