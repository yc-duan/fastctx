//! A conversation on another machine, followed there: each tree as that
//! machine tells it, or, from a machine before collaboration watches, as it
//! is read a moment after each read.

use super::{Sender, tell};
use crate::Node;
use fastctx_types::{
    ResidentId,
    access::{Acting, Purpose, Written},
    collab_prompt::{TREE_ACCOUNT, TREE_CAPABILITIES, TREE_MACHINE_OLD},
    collaboration::Follow,
    ipc::Failure,
    remote::{RemoteCall, RemoteReply},
    session::Watched,
};
use std::{sync::Arc, time::Duration};

/// How long after one read a machine that cannot be followed is read again.
const AGAIN: Duration = Duration::from_secs(2);

pub(super) async fn keep(
    node: &Arc<Node>,
    id: &ResidentId,
    machine: &Written,
    follow: Follow,
    acting: &Acting,
    told: &Sender,
) -> Failure {
    let speakers = match node.speakers(acting, machine, Purpose::Agents) {
        Ok(speakers) => speakers,
        Err(error) => return Failure::new(error.to_string()),
    };
    let request = fastctx_types::watching::Subscription {
        resident: id.clone(),
        composer: None,
        documents: None,
        held: None,
        now: false,
        goal: false,
        collaboration: Some(follow),
    };
    let mut refused = None;
    for (link, account) in speakers {
        let facts = match link.call(&machine.name, &account, Purpose::Look, RemoteCall::Facts).await {
            Ok(Ok(RemoteReply::Facts(facts))) => facts,
            Ok(Ok(_)) => return Failure::new(TREE_CAPABILITIES),
            Ok(Err(failure)) if failure.not_run => {
                refused = Some(failure);
                continue;
            }
            Ok(Err(failure)) => return failure,
            Err(error) => return Failure::new(error.to_string()),
        };
        if !facts.collaboration_tree {
            return Failure::not_run(TREE_MACHINE_OLD);
        }
        if !facts.collaboration_watch {
            return read_again(node, id, machine, follow, acting, told).await;
        }
        let mut watching = match link.watch_shared(&machine.name, &account, request.clone()).await {
            Ok(watching) => watching,
            Err(failure) if failure.not_run => {
                refused = Some(failure);
                continue;
            }
            Err(failure) => return failure,
        };
        loop {
            match watching.next().await {
                Ok(Some(Watched::Collaboration { tree, .. })) => tell(node, told, *tree),
                Ok(Some(_)) => {
                    return Failure::new("the conversation's machine told its history, not its collaboration");
                }
                Ok(None) => {
                    return Failure::unanswered("the conversation's machine ended the watch of its collaboration");
                }
                Err(failure) => return failure,
            }
        }
    }
    refused.unwrap_or_else(|| Failure::refused(TREE_ACCOUNT))
}

/// A machine from before collaboration watches: its tree read again a moment
/// after each read, told when it changed.
async fn read_again(
    node: &Arc<Node>,
    id: &ResidentId,
    machine: &Written,
    follow: Follow,
    acting: &Acting,
    told: &Sender,
) -> Failure {
    loop {
        match crate::colleagues::tree::remote(node, acting, machine, id.clone(), follow == Follow::Tree).await {
            Ok(tree) => tell(node, told, tree),
            Err(failure) => return failure,
        }
        tokio::time::sleep(AGAIN).await;
    }
}
