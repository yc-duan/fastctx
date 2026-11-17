//! A conversation's collaboration through a local node or a device's
//! authenticated route: read once, or followed as it changes.

use crate::{CallError, Error, Watching, World};
use fastctx_types::collab_prompt::{WATCH_MACHINE_OLD, WATCH_NODE_OLD};
use fastctx_types::{Call, Reply, ResidentId, access::Written, collaboration::Tree, session::Watched};
use std::time::Duration;

/// How long after one read a collaboration that cannot be followed is read
/// again.
const AGAIN: Duration = Duration::from_secs(2);

impl World {
    pub async fn collaboration(&self, resident: ResidentId, machine: Option<Written>) -> Result<Tree, Error> {
        match self.ask(Call::Collaboration { resident, machine }).await? {
            Reply::Collaboration(tree) => Ok(*tree),
            other => Err(Error::Unexpected(format!("{other:?}"))),
        }
    }

    /// Follows a conversation's collaboration: the tree as it is, then again
    /// each time it changes. Where the node, or for a device the
    /// conversation's machine, is from before collaboration watches, it is
    /// read again a moment after each read and told only when it changed.
    pub async fn watch_collaboration(
        &self,
        resident: ResidentId,
        machine: Option<Written>,
    ) -> Result<(CollaborationWatch, Tree), Error> {
        let opened = match self.device() {
            Some(device) => device.watch_collaboration(resident.clone(), machine.clone()).await.map(Watching::device),
            None => {
                let subscription = fastctx_types::watching::local::Subscription {
                    resident: resident.clone(),
                    machine: machine.clone(),
                    composer: None,
                    documents: None,
                    held: None,
                    now: false,
                    live_only: false,
                    goal: None,
                    collaboration: true,
                };
                self.local_watches.watch(self, subscription).await
            }
        };
        let how = match opened {
            Ok(watching) => How::Followed(watching),
            Err(error) if older(&error) => How::Read { world: self.clone(), resident, machine, last: None },
            Err(error) => return Err(error),
        };
        let mut watch = CollaborationWatch { how };
        let tree = watch
            .next()
            .await?
            .ok_or_else(|| Error::Unexpected("the collaboration watch ended before it told the tree".into()))?;
        Ok((watch, tree))
    }
}

/// Whether a watch was not opened because the node or the machine cannot
/// follow a collaboration: both are said on this side, before asking.
fn older(error: &Error) -> bool {
    matches!(error, Error::Call(CallError::Failed(failure))
        if failure.not_run && [WATCH_NODE_OLD, WATCH_MACHINE_OLD].contains(&failure.message.as_str()))
}

/// A conversation's collaboration as it changes (`World::watch_collaboration`).
pub struct CollaborationWatch {
    how: How,
}

enum How {
    Followed(Watching),
    Read { world: World, resident: ResidentId, machine: Option<Written>, last: Option<Box<Tree>> },
}

impl CollaborationWatch {
    /// The collaboration once it changed; `None` once the watch ended.
    pub async fn next(&mut self) -> Result<Option<Tree>, Error> {
        match &mut self.how {
            How::Followed(watching) => match watching.next().await? {
                Some(Watched::Collaboration { tree, .. }) => Ok(Some(*tree)),
                Some(_) => Err(Error::Unexpected("a collaboration watch carried a conversation's history".into())),
                None => Ok(None),
            },
            How::Read { world, resident, machine, last } => loop {
                if last.is_some() {
                    tokio::time::sleep(AGAIN).await;
                }
                let tree = world.collaboration(resident.clone(), machine.clone()).await?;
                if last.as_deref() != Some(&tree) {
                    *last = Some(Box::new(tree.clone()));
                    return Ok(Some(tree));
                }
            },
        }
    }
}
