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
        self.world_now_known(Vec::new()).await
    }

    /// Also decides visibility of machines retained by an interface across restarts.
    pub async fn world_now_known(&self, known: Vec<String>) -> Result<WorldNow, Error> {
        match self.ask(Call::WorldNow { known }).await? {
            Reply::WorldNow(world) => Ok(*world),
            other => Err(Error::Unexpected(format!("{other:?}"))),
        }
    }

    /// Follows the World: the first `next` gives it as it is, each later
    /// one as it changes.
    pub async fn watch_world(&self) -> Result<WorldWatch, Error> {
        self.watch_world_known(Vec::new()).await
    }

    /// Follows current permissions for both listed and previously known machines.
    pub async fn watch_world_known(&self, known: Vec<String>) -> Result<WorldWatch, Error> {
        let watching = match self.device() {
            Some(device) => Watching::Device {
                device: device.clone(),
                changes: device.world_changes(),
                first: true,
                told: None,
                unready: None,
                known,
            },
            None => {
                let (client, id) = self.connect().await?.watch_world(known).await?;
                Watching::Local { client, id, held: None }
            }
        };
        Ok(WorldWatch { watching })
    }
}

/// A watch of the World; dropping it ends the watch.
pub struct WorldWatch {
    watching: Watching,
}

/// A piece of a World watch: the World whole, or what changed in it since
/// the last piece (`fastctx_types::world_delta`).
pub enum Seen {
    World(Box<WorldNow>),
    Changed(serde_json::Value),
}

enum Watching {
    Local {
        client: Client,
        id: u64,
        /// The World as the node told it so far, to put what changed into.
        held: Option<serde_json::Value>,
    },
    Device {
        device: Arc<crate::device::Device>,
        changes: tokio::sync::watch::Receiver<u64>,
        first: bool,
        told: Option<Box<WorldNow>>,
        /// Since when the hubs have not all told what is afoot whole.
        unready: Option<tokio::time::Instant>,
        known: Vec<String>,
    },
}

/// How long a device holds the World back for what the hubs tell of the
/// machines to come whole: past it, it is told as it is known, and told
/// again as the rest comes, so that a session that never says all is told
/// cannot keep the interface from showing anything.
const READY_WITHIN: std::time::Duration = std::time::Duration::from_secs(3);

impl WorldWatch {
    /// The World, as it is at first, then after each change.
    pub async fn next(&mut self) -> Result<WorldNow, Error> {
        match self.next_seen().await? {
            Seen::World(world) => Ok(*world),
            Seen::Changed(_) => match &self.watching {
                Watching::Local { held: Some(held), .. } => serde_json::from_value(held.clone())
                    .map_err(|error| CallError::Broken(std::io::Error::other(error)).into()),
                _ => Err(CallError::Broken(std::io::Error::other("a change came before the World")).into()),
            },
        }
    }

    /// The World at first, then each change as the node told it: what
    /// changed, when it says only that, so it can be passed on as it is.
    pub async fn next_seen(&mut self) -> Result<Seen, Error> {
        let now = match &mut self.watching {
            Watching::Local { client, id, held } => match client.frame().await? {
                NodeFrame::World { id: told, world } if told == *id => {
                    *held = serde_json::to_value(&world).ok();
                    *world
                }
                NodeFrame::WorldChanged { id: told, change } if told == *id => {
                    let Some(held) = held.as_mut() else {
                        return Err(CallError::Broken(std::io::Error::other("a change came before the World")).into());
                    };
                    fastctx_types::world_delta::apply(held, &change);
                    return Ok(Seen::Changed(change));
                }
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
            Watching::Device { device, changes, first, told, unready, known } => loop {
                if !std::mem::take(first) {
                    let closed = || CallError::Broken(std::io::Error::other("the device closed"));
                    match unready.map(|since| since + READY_WITHIN) {
                        Some(until) => tokio::select! {
                            changed = changes.changed() => changed.map_err(|_| closed())?,
                            () = tokio::time::sleep_until(until) => {}
                        },
                        None => changes.changed().await.map_err(|_| closed())?,
                    }
                }
                changes.borrow_and_update();
                // The first answer may wait for afoot; later link and permission changes go at once.
                if told.is_some() || device.afoot_ready() {
                    *unready = None;
                } else if unready.get_or_insert_with(tokio::time::Instant::now).elapsed() < READY_WITHIN {
                    continue;
                }
                let now = device.world_now_known(known).await?;
                *known = now.machine_presence.iter().map(|machine| machine.machine.clone()).collect();
                if told.as_deref() != Some(&now) {
                    *told = Some(Box::new(now.clone()));
                    break now;
                }
            },
        };
        Ok(Seen::World(Box::new(now)))
    }
}
