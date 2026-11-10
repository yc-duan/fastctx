//! A conversation of this machine with the colleagues it asked: its own
//! branch as `branch` keeps it, and each colleague's, followed where the
//! colleague lives.

use super::{Following, SETTLE, Sender, Told, stopping, tell};
use crate::Node;
use crate::watch::Events;
use fastctx_net::watch_queue::TryRecvError;
use fastctx_types::{
    Address, ResidentId,
    access::{Acting, Written},
    collaboration::{Branch, Follow, Tree},
    ipc::Failure,
    session::{Happened, SessionEvent, What},
};
use std::{
    collections::BTreeMap,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::{Notify, watch};

/// The longest a colleague that could not be followed waits before it is
/// followed again.
const AGAIN: Duration = Duration::from_secs(30);

pub(super) async fn keep(node: &Arc<Node>, id: &ResidentId, acting: &Acting, told: &Sender) -> Failure {
    let owner = Address { machine: node.machine(), resident: id.clone() };
    let mut own = super::follow(node, id, None, Follow::Branch, acting);
    let mut events = node.watchers.watch_quietly(id);
    let mut residents = node.residents.changes();
    let changed = Arc::new(Notify::new());
    let mut colleagues: BTreeMap<Address, Colleague> = BTreeMap::new();
    // The colleagues its actions say it asked, read again when it may have
    // asked another.
    let mut asked: Option<BTreeMap<Address, String>> = None;
    let mut conversation: Option<Branch> = None;
    // When those could not be read, as when the hub that says whose a
    // colleague is cannot be reached for a moment, the colleagues known so
    // far stay, and they are read again a while later.
    let mut again: Option<Instant> = None;
    let mut retry = crate::again::Again::default();
    loop {
        let latest = own.borrow_and_update().clone();
        match latest {
            Some(Told::Tree { tree, .. }) => conversation = Some(tree.conversation.clone()),
            Some(Told::Ended(failure)) => return failure,
            None => {}
        }
        let mut found = match asked.clone() {
            Some(asked) => asked,
            None => match crate::colleagues::tree::asked(node, &owner, acting).await {
                Ok(found) => {
                    again = None;
                    retry.reset();
                    asked = Some(found.clone());
                    found
                }
                Err(_) => {
                    again = Some(Instant::now() + retry.next());
                    colleagues.values().map(|colleague| (colleague.address.clone(), colleague.label.clone())).collect()
                }
            },
        };
        found.extend(crate::colleagues::tree::born_here(node, &owner, acting));
        colleagues.retain(|address, _| found.contains_key(address));
        for (address, label) in found {
            if let std::collections::btree_map::Entry::Vacant(vacant) = colleagues.entry(address) {
                let address = vacant.key().clone();
                vacant.insert(Colleague::follow(node, &owner, address, label, acting, &changed));
            }
        }
        if let Some(conversation) = &conversation {
            let branches = colleagues
                .values()
                .map(|colleague| colleague.branch.borrow().clone().unwrap_or_else(|| colleague.reading()))
                .collect();
            tell(node, told, Tree { conversation: conversation.clone(), colleagues: branches });
        }
        let asks = tokio::select! {
            changed = own.changed() => if changed.is_err() { return stopping() } else { false },
            () = changed.notified() => false,
            event = events.recv() => match event {
                Ok(Some(event)) => may_have_asked(&event),
                Ok(None) | Err(_) => {
                    events = node.watchers.watch_quietly(id);
                    true
                }
            },
            changed = residents.changed() => if changed.is_err() { return stopping() } else { false },
            () = tokio::time::sleep_until(again.map_or_else(far, tokio::time::Instant::from_std)), if again.is_some() => true,
        };
        tokio::time::sleep(SETTLE).await;
        if asks || drain(node, id, &mut events) {
            asked = None;
        }
        residents.borrow_and_update();
    }
}

/// Whether the conversation may have asked another colleague: a call to ask
/// one ended, or a turn did.
fn may_have_asked(event: &SessionEvent) -> bool {
    match &event.happened {
        Happened::Finished { item, .. } => matches!(&item.what, What::Tool(tool) if tool.name == "ask_colleague"),
        Happened::TurnEnded { .. } => true,
        _ => false,
    }
}

/// Takes the events that came meanwhile: whether one may have asked a
/// colleague, or some were lost.
fn drain(node: &Node, id: &ResidentId, events: &mut Events) -> bool {
    let mut asks = false;
    loop {
        match events.try_recv() {
            Ok(event) => asks |= may_have_asked(&event),
            Err(TryRecvError::Empty) => return asks,
            Err(TryRecvError::Lagged | TryRecvError::Disconnected) => {
                *events = node.watchers.watch_quietly(id);
                return true;
            }
        }
    }
}

fn far() -> tokio::time::Instant {
    tokio::time::Instant::now() + Duration::from_secs(24 * 60 * 60)
}

/// A colleague as the tree follows it: its branch as last told, kept by a
/// task that follows it where it lives.
struct Colleague {
    address: Address,
    label: String,
    branch: watch::Receiver<Option<Branch>>,
    _following: Task,
}

impl Colleague {
    fn follow(
        node: &Arc<Node>,
        owner: &Address,
        address: Address,
        label: String,
        acting: &Acting,
        changed: &Arc<Notify>,
    ) -> Self {
        let (owner, acting) = (owner.clone(), acting.clone());
        let (sender, branch) = watch::channel(None);
        let (node, changed) = (node.clone(), changed.clone());
        let (following, unread) = (address.clone(), label.clone());
        let task = fastctx_platform::task::spawn(async move {
            match place(&node, &owner, &following, &acting).await {
                Ok((machine, acting)) => follow(node, following, unread, machine, acting, sender, changed).await,
                Err(failure) => {
                    sender.send_replace(Some(unread_branch(following, unread, failure.message)));
                    changed.notify_one();
                }
            }
        });
        Self { address, label, branch, _following: Task(task) }
    }

    fn reading(&self) -> Branch {
        let mut branch = unread_branch(self.address.clone(), self.label.clone(), String::new());
        branch.goal = Ok(None);
        branch.unavailable.clear();
        branch.reading = true;
        branch
    }
}

/// Where a colleague of `owner` is followed, and as whom: one asked through
/// an account as that account, on its hub.
async fn place(
    node: &Arc<Node>,
    owner: &Address,
    address: &Address,
    acting: &Acting,
) -> Result<(Option<Written>, Acting), Failure> {
    if node.is_me(&address.machine) {
        return Ok((None, acting.clone()));
    }
    let mut machine = Written::from(&address.machine);
    match crate::collab::asked_account(node, &owner.resident, address).await? {
        Some(account) => {
            machine.hub = node.link_of(&account.hub).map(|link| link.hub_name());
            Ok((Some(machine), Acting::one(account.hub, account.account)))
        }
        None => Ok((Some(machine), acting.clone())),
    }
}

/// Follows one colleague's branch, telling the tree each change; followed
/// again a while after it could not be.
async fn follow(
    node: Arc<Node>,
    address: Address,
    label: String,
    machine: Option<Written>,
    acting: Acting,
    sender: watch::Sender<Option<Branch>>,
    changed: Arc<Notify>,
) {
    let mut wait = Duration::from_secs(1);
    loop {
        let mut following: Following =
            super::follow(&node, &address.resident, machine.clone(), Follow::Branch, &acting);
        loop {
            match super::next(&mut following).await {
                Some(Ok((_, tree))) => {
                    let mut branch = tree.conversation;
                    if let Some(previous) = sender.borrow().as_ref() {
                        crate::colleagues::partial::retain_agents(&mut branch, previous);
                    }
                    sender.send_replace(Some(branch));
                    changed.notify_one();
                    wait = Duration::from_secs(1);
                }
                Some(Err(failure)) => {
                    let terminal = failure.is_refused() || failure.is_deleted();
                    let previous = sender.borrow().clone().filter(|_| !terminal);
                    let mut branch = unread_branch(address.clone(), label.clone(), failure.message);
                    if let Some(previous) = previous {
                        branch = Branch { reading: true, unavailable: branch.unavailable, ..previous };
                    }
                    sender.send_replace(Some(branch));
                    changed.notify_one();
                    if terminal {
                        return;
                    }
                    break;
                }
                None => return,
            }
        }
        drop(following);
        tokio::time::sleep(wait).await;
        wait = (wait * 2).min(AGAIN);
    }
}

/// A colleague whose branch could not be read, and why.
fn unread_branch(address: Address, label: String, why: String) -> Branch {
    Branch {
        address,
        label: Some(label),
        born_at: None,
        now: None,
        goal: Err(why.clone()),
        plan: None,
        agents: Vec::new(),
        reading: false,
        unavailable: vec![why],
    }
}

/// A task stopped when its holder lets it go.
struct Task(tokio::task::JoinHandle<()>);

impl Drop for Task {
    fn drop(&mut self) {
        self.0.abort();
    }
}
