//! How the machines and devices of an account stand in syncing with a hub,
//! asked from a device (design §28.9, §28.39): the hub reckons it; what the
//! device sees itself is whether the hub is reached. A device sends the hub
//! nothing of its own to hold, so nothing waits here to go up.

use fastctx_net::link::{Link, Status};
use fastctx_types::assets::{Reply as AssetReply, Request};
use fastctx_types::sync_status::LocalSync;
use fastctx_types::{AccountName, Reply};
use std::sync::Arc;

/// The answer to how `account`'s machines stand with the hub of `link`,
/// over the last `hours`.
pub(super) async fn of(
    link: &Arc<Link>,
    account: AccountName,
    hours: u32,
    machine: Option<fastctx_types::MachineName>,
) -> Reply {
    let (cut_off, why) = match link.status() {
        Status::Online { .. } => (None, None),
        Status::Connecting { since, last_error } => (Some(since.as_millisecond()), last_error),
        Status::Stopped { reason } => (Some(jiff::Timestamp::now().as_millisecond()), Some(reason)),
    };
    let local = LocalSync { machine: link.name(), unsent_frames: 0, unsent_bytes: 0, cut_off, why, unread: None };
    if cut_off.is_some() {
        return Reply::SyncStatus { local, hub: None, unreached: Some("the hub is not reached".into()) };
    }
    match link.assets(Request::SyncStatus { account, hours, machine }).await {
        Ok(AssetReply::SyncStatus { status }) => Reply::SyncStatus { local, hub: Some(status), unreached: None },
        Ok(other) => {
            Reply::SyncStatus { local, hub: None, unreached: Some(format!("the hub answered otherwise: {other:?}")) }
        }
        Err(why) => Reply::SyncStatus { local, hub: None, unreached: Some(why) },
    }
}
