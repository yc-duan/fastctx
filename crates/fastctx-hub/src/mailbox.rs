//! Session-facing mail commands. Each mailbox lives in its member's actor.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use crate::{Hub, post::Command};
use fastctx_types::{Address, MachineName};

const VIEW_TAKEN: Duration = Duration::from_millis(300);
const VIEW_UNASKED: Duration = Duration::from_secs(10);

#[derive(Clone)]
pub(crate) struct Backlog {
    pub member: MachineName,
    pub held: usize,
    pub writing: usize,
    pub pending: usize,
    pub on_way: usize,
}

pub(crate) fn backlog(hub: &Hub) -> Vec<Backlog> {
    let store = hub.store();
    store
        .book
        .directory
        .members
        .iter()
        .filter_map(|known| {
            let member = store.letters.member(&known.name)?;
            Some(Backlog {
                member: known.name.clone(),
                held: member.backlog.held.load(Ordering::Relaxed),
                writing: member.backlog.writing.load(Ordering::Relaxed),
                pending: member.backlog.pending.load(Ordering::Relaxed),
                on_way: member.backlog.on_way.load(Ordering::Relaxed),
            })
        })
        .collect()
}

pub(crate) fn view_told(hub: &Arc<Hub>, name: &MachineName, generation: u64) {
    release(hub, name, generation, VIEW_TAKEN);
}
pub(crate) fn opened(hub: &Arc<Hub>, name: &MachineName, generation: u64) {
    release(hub, name, generation, VIEW_UNASKED);
}
fn release(hub: &Arc<Hub>, name: &MachineName, generation: u64, after: Duration) {
    let member = hub.store().letters.member(name);
    fastctx_platform::task::spawn(async move {
        tokio::time::sleep(after).await;
        if let Some(member) = member {
            let _ = member.tell(Command::Viewed { generation }).await;
        }
    });
}

pub(crate) fn taken(hub: &Arc<Hub>, name: &MachineName, generation: u64, through: u64) {
    if let Some(member) = hub.store().letters.member(name) {
        member.later(Command::Taken { generation, through });
    }
}

pub(crate) async fn drop_for(hub: &Arc<Hub>, conversation: &Address) -> std::io::Result<usize> {
    let Some(member) = hub.store().letters.member(&conversation.machine) else { return Ok(0) };
    member
        .request(|reply| Command::DropFor { resident: conversation.resident.clone(), reply })
        .await
        .map_err(std::io::Error::other)
}
