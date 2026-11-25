//! Whole-machine shares that wait for their machine to clear (sync v2
//! design §6.3, R07): a share that gives an account whole access it did not
//! have is written waiting, at the machine's holding revision. From then the
//! machine holds as if it were in force — it is sent nothing the new holder
//! may not see, and clears what it has — while the share gives nothing: no
//! work on the machine, nothing read through it. The machine says, under its
//! key, that it cleared at that revision; the share is in force from then,
//! if the revision is still the machine's. A holder added meanwhile, or any
//! change to what decides what the machine may hold, moves the revision on,
//! and the machine clears again; nothing else does.

use crate::Hub;
use fastctx_net::directory::Directory;
use fastctx_net::sign::{Domain, PublicKey, Signature};
use fastctx_types::MachineName;
use fastctx_types::assets::Reply;
use fastctx_types::machine_log::{Action, Actor, Outcome};
use sha2::{Digest, Sha256};
use std::sync::Arc;

/// A share about to wait, at the revision the save that writes it gives.
pub(crate) const UNSET: u64 = 0;

/// What decides what any machine may hold (design §6.3): who each machine
/// is and acts for, the accounts, the shares in force or waiting, assistance,
/// acting and the projects — not whether a machine is there, nor any
/// revision itself. A save that changes none of it moves no revision on.
pub(crate) fn basis(directory: &Directory) -> String {
    // Each entry as one line, the lines sorted: the same directory in
    // another order is the same basis.
    let mut lines: Vec<String> = Vec::new();
    for member in &directory.members {
        let mut was: Vec<&str> = member.was.iter().map(|name| name.as_str()).collect();
        was.sort_unstable();
        let mut logins: Vec<&str> = member.logins.iter().map(|login| login.as_str()).collect();
        logins.sort_unstable();
        lines.push(
            serde_json::json!([
                "member",
                member.name.as_str(),
                was,
                crate::assets::witness::hex(member.key.as_bytes()),
                format!("{:?}", member.kind),
                logins,
                member.home.as_ref().map(|home| home.as_str()),
                member.managed
            ])
            .to_string(),
        );
    }
    for account in &directory.accounts {
        lines.push(
            serde_json::json!([
                "account",
                account.name.as_str(),
                format!("{:?}", account.level),
                account.locked,
                account.opened.as_millisecond()
            ])
            .to_string(),
        );
    }
    for share in &directory.shares {
        lines.push(
            serde_json::json!(["share", share.machine.as_str(), share.account.as_str(), share.active()]).to_string(),
        );
    }
    for assist in &directory.assists {
        lines.push(serde_json::json!(["assist", &**assist]).to_string());
    }
    for acting in &directory.acting {
        lines.push(serde_json::json!(["acting", &**acting]).to_string());
    }
    for project in &directory.projects {
        lines.push(serde_json::json!(["project", &**project]).to_string());
    }
    lines.sort_unstable();
    let mut hasher = Sha256::new();
    for line in &lines {
        hasher.update((line.len() as u64).to_le_bytes());
        hasher.update(line.as_bytes());
    }
    crate::assets::witness::hex(&hasher.finalize()[..16])
}

/// The revision a save that takes the chain's number `seq` gives: past
/// every revision an earlier save gave, whatever the clock says.
fn revision_at(seq: u64) -> u64 {
    (seq << 20) | (jiff::Timestamp::now().as_microsecond().max(0) as u64 & 0xF_FFFF)
}

/// Every share of `machine` waiting, and one about to, waits anew: at the
/// revision of the save that writes it, and its machine clears again.
pub(crate) fn wait_anew(directory: &mut Directory, machine: &MachineName) {
    for share in directory.shares.iter_mut().filter(|share| share.machine == *machine && share.pending.is_some()) {
        Arc::make_mut(share).pending = Some(UNSET);
    }
}

/// The save about to take the chain's number `seq`: every share waiting at
/// no revision yet, or at one whose basis the directory no longer has,
/// waits at a revision of this save, and its machine clears again (design
/// §6.3, N04); a save that changes nothing that decides what a machine may
/// hold moves nothing on. Whether any moved.
pub(crate) fn renew(directory: &mut Directory, seq: u64) -> bool {
    if !directory.shares.iter().any(|share| share.pending.is_some()) {
        return false;
    }
    let basis = basis(directory);
    let machines: std::collections::BTreeSet<MachineName> = directory
        .shares
        .iter()
        .filter(|share| {
            share.pending.is_some() && (share.pending == Some(UNSET) || share.basis.as_deref() != Some(basis.as_str()))
        })
        .map(|share| share.machine.clone())
        .collect();
    let revision = revision_at(seq);
    for share in
        directory.shares.iter_mut().filter(|share| share.pending.is_some() && machines.contains(&share.machine))
    {
        let share = Arc::make_mut(share);
        share.pending = Some(revision);
        share.basis = Some(basis.clone());
    }
    !machines.is_empty()
}

/// `machine`, whose key is `key`, says it cleared at `revision`: the shares
/// of it waiting at that revision are in force from now. Call on a thread
/// that may block.
pub(crate) fn cleared(
    hub: &Hub,
    machine: &MachineName,
    key: &PublicKey,
    revision: u64,
    proof: &[u8],
) -> Result<Reply, String> {
    let proof = <[u8; 64]>::try_from(proof).map_err(|_| "a proof is 64 bytes".to_string())?;
    let statement = fastctx_types::assets::cleared_statement(machine.as_str(), revision);
    key.verify(Domain::ShareCleared, &statement, &Signature::from_bytes(proof))
        .map_err(|_| "the machine's word that it cleared does not verify".to_string())?;
    // Sealed, the hub grants nothing: the seal is held while this commits.
    let seal = hub.assets.seal_held();
    if seal.is_some() {
        return Err("every asset is sealed until the hub is reopened: no access is granted meanwhile".into());
    }
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
    for share in store
        .book
        .directory
        .shares
        .iter_mut()
        .filter(|share| share.machine == machine && share.pending == Some(revision))
    {
        Arc::make_mut(share).pending = None;
    }
    store.book.directory.settle_acting(&before.directory);
    let actor = Actor { account: None, authenticated: true, member: Some(machine.clone()), peer: None };
    let result = (|| {
        for (index, share) in waiting.iter().enumerate() {
            let action =
                Action::Share { machine: machine.clone(), to: share.account.clone(), shared: true, jobs: share.jobs };
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
        // This very save puts them in force; any other machine's share whose
        // basis that moves waits at a new revision, told with this change.
        store.save_book().map_err(|error| error.to_string())
    })();
    if let Err(error) = result {
        store.book = before;
        return Err(error);
    }
    Ok(Reply::Cleared { activated: waiting.len() as u32 })
}
