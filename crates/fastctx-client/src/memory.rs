//! Project memory through a node or directly from a device's hub connection.

use crate::{CallError, Client, Error, World};
use fastctx_types::{
    AccountName, Call, HubName, Reply, TownName,
    ipc::NodeFrame,
    memory::{Catalog, MemoryCall, MemoryReply, Settings},
};
use std::sync::Arc;

impl World {
    pub async fn memory(&self, request: MemoryCall) -> Result<MemoryReply, Error> {
        match self.ask(Call::Memory(request)).await? {
            Reply::Memory(reply) => Ok(reply),
            other => Err(Error::Unexpected(format!("{other:?}"))),
        }
    }

    pub async fn memory_settings(
        &self,
        hub: Option<HubName>,
        account: Option<AccountName>,
        enabled: Option<bool>,
    ) -> Result<Settings, Error> {
        match self.ask(Call::MemorySettings { hub, account, enabled }).await? {
            Reply::MemorySettings(settings) => Ok(settings),
            other => Err(Error::Unexpected(format!("{other:?}"))),
        }
    }

    pub async fn watch_memory(&self, town: TownName) -> Result<MemoryWatch, Error> {
        if let Some(device) = self.device() {
            return Ok(MemoryWatch::Device {
                device: device.clone(),
                town,
                changes: device.world_changes(),
                first: true,
                told: None,
            });
        }
        let (client, id) = self.connect().await?.watch_memory(town).await?;
        Ok(MemoryWatch::Local { client, id })
    }
}

pub enum MemoryWatch {
    Local {
        client: Client,
        id: u64,
    },
    Device {
        device: Arc<crate::device::Device>,
        town: TownName,
        changes: tokio::sync::watch::Receiver<u64>,
        first: bool,
        told: Option<Catalog>,
    },
}

impl MemoryWatch {
    pub async fn next(&mut self) -> Result<Catalog, Error> {
        match self {
            Self::Local { client, id } => match client.frame().await? {
                NodeFrame::Reply { id: reply_id, reply } if reply_id == *id => match reply {
                    Ok(Reply::Memory(MemoryReply::Catalog { catalog })) => Ok(catalog),
                    Err(error) => Err(CallError::Failed(error).into()),
                    other => Err(Error::Unexpected(format!("{other:?}"))),
                },
                other => Err(Error::Unexpected(format!("{other:?}"))),
            },
            Self::Device { device, town, changes, first, told } => loop {
                if !std::mem::take(first) {
                    changes
                        .changed()
                        .await
                        .map_err(|_| CallError::Broken(std::io::Error::other("the device closed")))?;
                }
                changes.borrow_and_update();
                let (reply, _) = device.call(Call::Memory(MemoryCall::List { town: town.clone() })).await?;
                let Reply::Memory(MemoryReply::Catalog { catalog }) = reply else {
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
