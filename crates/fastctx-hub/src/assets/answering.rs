//! A device's asset request answered whole: by the directory as it stood
//! when the request began, judged again by the directory as it stands once
//! the answer is made, with what the answer gives other machines to be
//! told. The control connection and the asset lines both answer here.

use super::serve::{self, Room};
use crate::Hub;
use crate::origin::Origin;
use fastctx_types::MachineName;
use fastctx_types::assets::{Push, Reply, Request};
use std::sync::Arc;

/// Answers `request` of the member `origin`, come with `room`; and whom to
/// tell what unasked once it is answered. Call on a thread that may block.
pub(crate) fn answer(
    hub: &Arc<Hub>,
    origin: &Origin,
    request: Request,
    room: Room,
) -> (Result<Reply, String>, Vec<(MachineName, Push)>) {
    let from = &origin.name;
    // A machine's word that it cleared changes the directory: it is taken
    // under the directory's lock.
    if let Request::Cleared { revision, proof } = &request {
        let answer = origin
            .check(&hub.store().book.directory)
            .and_then(|()| crate::shares::cleared(hub, from, &origin.key, *revision, &proof.0));
        // Shares put in force are told like any change.
        if matches!(answer, Ok(Reply::Cleared { activated }) if activated > 0) {
            hub.changed();
        }
        return (answer, Vec::new());
    }
    // How an account's machines stand is reckoned beside the sessions, where
    // who is online is known.
    if let Request::SyncStatus { account, hours, machine } = &request {
        let answer = origin
            .check(&hub.store().book.directory)
            .and_then(|()| super::standing::status(hub, from, account, *hours, machine.as_ref()));
        return (answer, Vec::new());
    }
    let scope = serve::scope_of(&request).map(str::to_string);
    let stream = serve::stream_of(&request).map(str::to_string);
    let store = hub.store();
    let answer = origin.check(&store.book.directory).and_then(|()| {
        let basis = || store.basis().to_string();
        serve::ask_in(&hub.assets, &store.indexed(), &basis, from, &origin.key, request, room)
    });
    drop(store);
    // Judged again, and told only, by the directory as it stands now, read
    // after what is to be told was taken.
    let drained = hub.assets.drain_pushes();
    let now = hub.store();
    let answer = match (answer, scope) {
        (Ok(reply), Some(key)) => serve::recheck(&hub.assets, &now.indexed(), from, &key, stream.as_deref(), reply),
        (Ok(reply @ Reply::Holdings { .. }), None) => {
            serve::recheck_holdings(&hub.assets, &now.indexed(), now.basis(), from, reply)
        }
        (answer, _) => answer,
    };
    let pushes = hub.assets.take_pushes(drained, &now.indexed());
    (answer, pushes)
}
