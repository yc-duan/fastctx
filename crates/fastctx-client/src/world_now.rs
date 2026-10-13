//! The World as an interface shows it (`fastctx_types::world::WorldNow`),
//! from what this computer's node holds, or what a device's own links hold,
//! and followed as it changes: nothing is asked of any machine for it
//! (`world-network.md`, 近况).

use std::sync::Arc;

use fastctx_types::ipc::NodeFrame;
use fastctx_types::world::WorldNow;
use fastctx_types::{Call, Reply};

use crate::{CallError, Client, Error, World};

impl World {
    /// The World as it is now.
    pub async fn world_now(&self) -> Result<WorldNow, Error> {
        match self.ask(Call::WorldNow).await? {
            Reply::WorldNow(world) => Ok(*world),
            other => Err(Error::Unexpected(format!("{other:?}"))),
        }
    }

    /// Follows the World: the first `next` gives it as it is, each later
    /// one as it changes.
    pub async fn watch_world(&self) -> Result<WorldWatch, Error> {
        let watching = match self.device() {
            Some(device) => {
                Watching::Device { device: device.clone(), changes: device.world_changes(), first: true, told: None }
            }
            None => {
                let (client, id) = self.connect().await?.watch_world().await?;
                Watching::Local { client, id }
            }
        };
        Ok(WorldWatch { watching })
    }
}

/// A watch of the World; dropping it ends the watch.
pub struct WorldWatch {
    watching: Watching,
}

enum Watching {
    Local {
        client: Client,
        id: u64,
    },
    Device {
        device: Arc<crate::device::Device>,
        changes: tokio::sync::watch::Receiver<u64>,
        first: bool,
        told: Option<Box<WorldNow>>,
    },
}

impl WorldWatch {
    /// The World, as it is at first, then after each change.
    pub async fn next(&mut self) -> Result<WorldNow, Error> {
        let now = match &mut self.watching {
            Watching::Local { client, id } => match client.frame().await? {
                NodeFrame::World { id: told, world } if told == *id => *world,
                NodeFrame::Reply { id: told, reply } if told == *id => {
                    return Err(match reply {
                        Ok(_) => CallError::Broken(std::io::Error::other("the World watch ended")),
                        Err(failure) => CallError::Failed(failure),
                    }
                    .into());
                }
                _ => {
                    return Err(CallError::Broken(std::io::Error::other(
                        "the node answered another call on a World watch",
                    ))
                    .into());
                }
            },
            Watching::Device { device, changes, first, told } => loop {
                if !std::mem::take(first) {
                    changes
                        .changed()
                        .await
                        .map_err(|_| CallError::Broken(std::io::Error::other("the device closed")))?;
                }
                changes.borrow_and_update();
                // Until the hubs have told what is afoot again, what was
                // told before stands.
                if !device.afoot_ready() {
                    continue;
                }
                let now = device.world_now().await?;
                if told.as_deref() != Some(&now) {
                    *told = Some(Box::new(now.clone()));
                    break now;
                }
            },
        };
        Ok(now)
    }
}
