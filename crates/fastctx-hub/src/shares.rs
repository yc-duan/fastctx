//! Whole-machine shares that wait for their machine to clear (sync v2
//! design §6.3, R07): a share that gives an account whole access it did not
//! have is written waiting, at the machine's holding revision. From then the
//! machine holds as if it were in force — it is sent nothing the new holder
//! may not see, and clears what it has — while the share gives nothing: no
//! work on the machine, nothing read through it. The machine says, under its
//! key, that it cleared at that revision; the share is in force from then,
//! if the revision is still the machine's. A holder added meanwhile moves the
//! revision on, and the machine clears again.

use crate::Hub;
use fastctx_net::directory::Directory;
use fastctx_net::sign::{Domain, PublicKey, Signature};
use fastctx_types::MachineName;
use fastctx_types::assets::Reply;
use fastctx_types::machine_log::{Action, Actor, Outcome};
use std::sync::Arc;

/// A holding revision of `machine` past every one its shares wait at.
pub(crate) fn next_revision(directory: &Directory, machine: &MachineName) -> u64 {
    let waiting = directory.shares.iter().filter(|share| share.machine == *machine).filter_map(|share| share.pending).max();
    let now = jiff::Timestamp::now().as_millisecond().max(0) as u64;
    waiting.map_or(now, |waiting| now.max(waiting + 1))
}

/// Every share of `machine` still waiting waits at `revision` now.
pub(crate) fn wait_again(directory: &mut Directory, machine: &MachineName, revision: u64) {
    for share in directory.shares.iter_mut().filter(|share| share.machine == *machine && share.pending.is_some()) {
        Arc::make_mut(share).pending = Some(revision);
    }
}

/// `machine`, whose key is `key`, says it cleared at `revision`: the shares
/// of it waiting at that revision are in force from now. Call on a thread
/// that may block.
pub(crate) fn cleared(hub: &Hub, machine: &MachineName, key: &PublicKey, revision: u64, proof: &[u8]) -> Result<Reply, String> {
    let proof = <[u8; 64]>::try_from(proof).map_err(|_| "a proof is 64 bytes".to_string())?;
    let statement = fastctx_types::assets::cleared_statement(machine.as_str(), revision);
    key.verify(Domain::ShareCleared, &statement, &Signature::from_bytes(proof))
        .map_err(|_| "the machine's word that it cleared does not verify".to_string())?;
    let mut store = hub.edit_store();
    let machine = store.book.directory.canonical(machine).clone();
    let waiting: Vec<_> = store
        .book
        .directory
        .shares
        .iter()
        .filter(|share| share.machine == machine && share.pending == Some(revision))
        .cloned()
        .collect();
    if waiting.is_empty() {
        // Cleared at a revision since moved on, or already in force.
        return Ok(Reply::Cleared { activated: 0 });
    }
    let before = store.book.clone();
    for share in store.book.directory.shares.iter_mut().filter(|share| share.machine == machine && share.pending == Some(revision)) {
        Arc::make_mut(share).pending = None;
    }
    store.book.directory.settle_acting(&before.directory);
    let actor = Actor { account: None, authenticated: true, member: Some(machine.clone()), peer: None };
    let result = (|| {
        for (index, share) in waiting.iter().enumerate() {
            let action = Action::Share { machine: machine.clone(), to: share.account.clone(), shared: true, jobs: share.jobs };
            if index == 0 {
                crate::machine_log::effects(&mut store, &before.directory, &actor, &action)?;
            }
            let accounts = store.book.directory.accounts.clone();
            store
                .book
                .machine_log
                .push(&before.directory, &accounts, actor.clone(), action, Outcome::Done)
                .map_err(|error| error.to_string())?;
        }
        store.book_edit = crate::store::BookEdit::Full;
        store.save_book().map_err(|error| error.to_string())
    })();
    if let Err(error) = result {
        store.book = before;
        return Err(error);
    }
    Ok(Reply::Cleared { activated: waiting.len() as u32 })
}
