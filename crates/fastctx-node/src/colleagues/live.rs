//! A conversation's collaboration as the node keeps it while someone follows
//! it (`session::Watched::Collaboration`): read once, then kept by what
//! happens. The conversation's events fold into its native agents, the
//! residents' changes tell how it stands, and each colleague is followed
//! where it lives. What several follow is kept once for them all, and only
//! while one does.

mod branch;
mod elsewhere;
mod tree;

use crate::Node;
use fastctx_net::watch_queue;
use fastctx_types::{
    ResidentId,
    access::{Acting, Written},
    collaboration::{Follow, Tree},
    ipc::Failure,
    session::Watched,
};
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::sync::watch;

/// How long after a change the collaboration is worked out again, gathering
/// the changes that come with it: an agent at work tells many a second.
const SETTLE: Duration = Duration::from_millis(250);

/// What those who follow a collaboration are told: the latest tree, or why
/// it can no longer be followed.
#[derive(Clone)]
pub(crate) enum Told {
    Tree { version: u64, tree: Arc<Tree> },
    Ended(Failure),
}

type Sender = Arc<watch::Sender<Option<Told>>>;
pub(crate) type Following = watch::Receiver<Option<Told>>;

/// The collaborations followed from this node, by what is followed and as
/// whom.
#[derive(Default)]
pub(crate) struct Followed {
    kept: std::sync::Mutex<HashMap<String, Sender>>,
    /// The version of the latest tree told, higher with each.
    version: AtomicU64,
}

/// What one keeper keeps: `id`'s collaboration as `acting` sees it, on
/// `machine` when it is not this one.
#[derive(Clone)]
struct Key {
    id: ResidentId,
    machine: Option<Written>,
    follow: Follow,
    acting: Acting,
}

impl Key {
    fn name(&self) -> String {
        serde_json::to_string(&(&self.id, &self.machine, self.follow, &self.acting)).expect("plain values")
    }
}

/// Follows `id`'s collaboration as `acting` sees it, on `machine` when it
/// lives elsewhere: what is told comes on the returned receiver, nothing
/// until the first tree is worked out. What was told already counts as a
/// change, so its first `changed` returns at once.
pub(crate) fn follow(
    node: &Arc<Node>,
    id: &ResidentId,
    machine: Option<Written>,
    follow: Follow,
    acting: &Acting,
) -> Following {
    let key = Key { id: id.clone(), machine, follow, acting: acting.clone() };
    let name = key.name();
    let mut kept = node.collaboration_followed.kept.lock().expect("never poisoned");
    let mut following = match kept.get(&name) {
        Some(told) => told.subscribe(),
        None => {
            let (told, following) = watch::channel(None);
            let told = Arc::new(told);
            kept.insert(name.clone(), told.clone());
            let node = node.clone();
            fastctx_platform::task::spawn(async move { keep(node, key, name, told).await });
            following
        }
    };
    following.mark_changed();
    following
}

/// Keeps a collaboration for as long as one follows it, or until it can no
/// longer be followed.
async fn keep(node: Arc<Node>, key: Key, name: String, told: Sender) {
    let what = match (&key.machine, key.follow) {
        (None, Follow::Tree) => format!("{}'s collaboration", key.id),
        (None, Follow::Branch) => format!("{}'s own collaboration", key.id),
        (Some(machine), _) => format!("{}'s collaboration on {machine}", key.id),
    };
    let started = std::time::Instant::now();
    node.log.line(format_args!("following {what}"));
    loop {
        let ended = tokio::select! {
            failure = work(&node, &key, &told) => Some(failure),
            () = told.closed() => None,
        };
        let lasted = crate::talk::lasted(started);
        match &ended {
            Some(failure) => {
                node.log.line(format_args!("stopped following {what} after {lasted}: {}", failure.message))
            }
            None => node.log.line(format_args!("stopped following {what} after {lasted}: no one follows it")),
        }
        let mut kept = node.collaboration_followed.kept.lock().expect("never poisoned");
        match ended {
            Some(failure) => {
                told.send_replace(Some(Told::Ended(failure)));
            }
            // One came to follow it as the last one left.
            None if told.receiver_count() > 0 => continue,
            None => {}
        }
        if kept.get(&name).is_some_and(|current| Arc::ptr_eq(current, &told)) {
            kept.remove(&name);
        }
        return;
    }
}

async fn work(node: &Arc<Node>, key: &Key, told: &Sender) -> Failure {
    match (&key.machine, key.follow) {
        (None, Follow::Branch) => branch::keep(node, &key.id, &key.acting, told).await,
        (None, Follow::Tree) => tree::keep(node, &key.id, &key.acting, told).await,
        (Some(machine), follow) => elsewhere::keep(node, &key.id, machine, follow, &key.acting, told).await,
    }
}

/// Tells those who follow `tree`, when it is not what they were told last.
fn tell(node: &Node, told: &Sender, tree: Tree) {
    told.send_if_modified(|current| {
        if matches!(current, Some(Told::Tree { tree: last, .. }) if **last == tree) {
            return false;
        }
        let version = node.collaboration_followed.version.fetch_add(1, Ordering::Relaxed) + 1;
        *current = Some(Told::Tree { version, tree: Arc::new(tree) });
        true
    });
}

/// The failure that ends a keeper as the node stops.
fn stopping() -> Failure {
    Failure::new("the node is stopping")
}

/// Watches a conversation's collaboration on any machine for the user here
/// (`Call::Watch::collaboration`): the tree, then again each time it
/// changes, until the watch is dropped or the collaboration can no longer
/// be followed.
pub(crate) async fn watch(
    node: &Arc<Node>,
    resident: ResidentId,
    machine: Option<Written>,
) -> Result<watch_queue::Receiver<Result<Watched, Failure>>, Failure> {
    let here = machine.is_none() && node.residents.about(&resident).is_some();
    let elsewhere = match machine {
        _ if here => None,
        Some(machine) if node.is_here(&machine) => None,
        Some(machine) => Some(machine),
        None => Some(crate::talk::locate(node, &resident).await?).filter(|machine| !node.is_here(machine)),
    };
    let mut following = follow(node, &resident, elsewhere, Follow::Tree, &Acting::Here);
    let (sender, receiver) = watch_queue::channel(|piece: &Result<Watched, Failure>| match piece {
        Ok(piece) => watch_queue::watched_size(piece),
        Err(error) => (error.message.len(), None),
    });
    fastctx_platform::task::spawn(async move {
        loop {
            tokio::select! {
                changed = following.changed() => if changed.is_err() { return },
                () = sender.closed() => return,
            }
            let told = following.borrow_and_update().clone();
            match told {
                Some(Told::Tree { version, tree }) => {
                    let tree = Box::new(Arc::unwrap_or_clone(tree));
                    if sender.send(Ok(Watched::Collaboration { version, tree })).is_err() {
                        return;
                    }
                }
                Some(Told::Ended(failure)) => {
                    let _ = sender.send(Err(failure));
                    return;
                }
                None => {}
            }
        }
    });
    Ok(receiver)
}

/// The next tree `following` tells after what was seen of it: `Err` when the
/// collaboration can no longer be followed, `None` when its keeper went
/// with the node.
pub(crate) async fn next(following: &mut Following) -> Option<Result<(u64, Tree), Failure>> {
    loop {
        following.changed().await.ok()?;
        let told = following.borrow_and_update().clone();
        match told {
            Some(Told::Tree { version, tree }) => return Some(Ok((version, Arc::unwrap_or_clone(tree)))),
            Some(Told::Ended(failure)) => return Some(Err(failure)),
            None => {}
        }
    }
}

/// Hands the collaboration of a conversation of this machine to whoever
/// asked through `link`, over `stream`, until they close it: each tree,
/// signed here, as it changes.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn serve(
    node: &Arc<Node>,
    asker: &crate::admission::Asker,
    link: &Arc<fastctx_net::link::Link>,
    from: &fastctx_types::MachineName,
    resident: ResidentId,
    watch: u64,
    follow: Follow,
    mut stream: fastctx_net::data::Stream,
) -> Result<fastctx_types::remote::RemoteReply, Failure> {
    let what = format!("{from}'s watch of {resident}'s collaboration");
    let started = std::time::Instant::now();
    let ended = |why: &str| {
        node.log.line(format_args!("{what} ended after {}: {why}", crate::talk::lasted(started)));
        Ok(fastctx_types::remote::RemoteReply::Watched)
    };
    let acting = Acting::one(asker.hub.clone(), asker.account.clone());
    let mut following = self::follow(node, &resident, None, follow, &acting);
    node.log.line(format_args!("{what} opened"));
    let mut n = 0;
    loop {
        let told = tokio::select! {
            told = next(&mut following) => told,
            why = stream.ended() => return ended(why.as_deref().unwrap_or("it closed the watch")),
        };
        match told {
            Some(Ok((version, tree))) => {
                let watched = Watched::Collaboration { version, tree: Box::new(tree) };
                let piece = crate::talk::Piece { watch, n, watched };
                let signed = link.sign(fastctx_net::sign::Domain::Event, &piece);
                if let Err(why) = fastctx_net::watch_stream::send(&mut stream, &signed).await {
                    return ended(&format!("sending: {why}"));
                }
                n += 1;
            }
            Some(Err(failure)) => {
                // Closed rather than dropped, so the watcher waits for the
                // answer that says why.
                stream.close().await;
                return Err(failure);
            }
            None => return ended("the node stopped"),
        }
    }
}
