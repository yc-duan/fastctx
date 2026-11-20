//! A conversation's scopes going with the conversation (design §6.2).
//!
//! When the machine a conversation lived on has deleted it, the hub
//! describes its scopes no more. Their rows go, and what the hub's own
//! history kept of them; the chain keeps that they were deleted; every
//! machine holding one is told it holds it no more; and the owner library
//! lets go of what it kept of them, a part at a time.
//!
//! A scope deleted so is answered, whatever is asked of it, as one read no
//! more: a machine that was away drops its copy when it next asks after it.
//! Described again, it is refused, so no copy left somewhere brings it back.

use super::owners::Owner;
use super::{Assets, chain, scopes};
use fastctx_storage::rusqlite::params;
use fastctx_types::assets::{Push, Readers};
use fastctx_types::{Address, MachineName};
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashSet};
use std::io;
use std::sync::{Arc, Mutex, OnceLock};

/// What the chain calls a scope's deletion.
const KIND: &str = "scope-deleted";
/// The passes an owner library is given at a time to let go of what it
/// kept, the rows of each kind a pass takes, and the pause between two: its
/// writer serves requests meanwhile, and a frame's body goes with its row.
const PASSES_AT_ONCE: usize = 64;
const ROWS_A_PASS: i64 = 16;
const SCOPES_A_PASS: i64 = 2;
const PACE: std::time::Duration = std::time::Duration::from_millis(20);
/// The longest a turn of all the libraries together takes.
const TURN: std::time::Duration = std::time::Duration::from_secs(2);

/// The scopes deleted with their conversations, and the owner libraries
/// still letting go of them.
#[derive(Default)]
pub(crate) struct Deleted {
    /// What the chain keeps of each, read from it once.
    keys: OnceLock<Arc<Mutex<HashSet<[u8; 32]>>>>,
    purging: Mutex<BTreeSet<Owner>>,
    /// Those whose owner library could not be reached as they were deleted:
    /// tried again at each turn (`let_go`).
    unreached: Mutex<Vec<(Owner, String)>>,
    /// The owner libraries that could not be looked through as they opened
    /// for scopes deleted and still in them: looked through at a turn.
    unlooked: Mutex<BTreeSet<Owner>>,
    /// Held by the one turn under way.
    turn: Mutex<()>,
    /// The conversations whose scopes are being deleted now (none), and when
    /// the last try of each that failed ended: tried again no sooner than
    /// `AGAIN` after (`conversation_gone`).
    trying: Mutex<std::collections::HashMap<String, Option<std::time::Instant>>>,
}

/// How long after a try to delete a conversation's scopes failed it is tried
/// again.
const AGAIN: std::time::Duration = std::time::Duration::from_secs(60);

/// The scopes deleted, as a write to the control database looks them up
/// (`Assets::deleted_scopes`).
pub(super) type Stamps = Arc<Mutex<HashSet<[u8; 32]>>>;

/// What the chain keeps of the deletion of the scope `key`.
pub(super) fn stamp(key: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"fastctx-hub-scope-deleted/1");
    hasher.update(key.as_bytes());
    hasher.finalize().into()
}

fn other(error: impl std::fmt::Display) -> io::Error {
    io::Error::other(error.to_string())
}

impl Assets {
    /// The scopes deleted, for a write to the control database to look a
    /// key up in as it runs: a deletion marks its scope here before its own
    /// write is asked for, and the database writes one at a time, so a write
    /// that runs after a deletion's finds its scope marked.
    pub(super) fn deleted_scopes(&self) -> io::Result<Stamps> {
        self.deleted_keys().cloned()
    }

    fn deleted_keys(&self) -> io::Result<&Stamps> {
        if let Some(keys) = self.deleted.keys.get() {
            return Ok(keys);
        }
        let kept: Vec<Vec<u8>> = self
            .control
            .read_now(|c| {
                let mut statement = c.prepare_cached("SELECT digest FROM hub_chain WHERE kind = ?1")?;
                let rows = statement.query_map([KIND], |row| row.get(0))?;
                Ok(rows.collect::<std::result::Result<_, _>>()?)
            })
            .map_err(io::Error::other)?;
        let kept = kept.into_iter().filter_map(|digest| <[u8; 32]>::try_from(digest).ok()).collect();
        Ok(self.deleted.keys.get_or_init(|| Arc::new(Mutex::new(kept))))
    }

    /// Whether the scope `key` was deleted with its conversation.
    pub(crate) fn was_deleted(&self, key: &str) -> bool {
        self.deleted_keys().is_ok_and(|keys| {
            let keys = keys.lock().expect("deleted scopes");
            !keys.is_empty() && keys.contains(&stamp(key))
        })
    }

    /// Whether to try deleting the scopes of the conversation `key` now: not
    /// while a try is under way, nor soon after one failed.
    fn try_conversation(&self, key: &str) -> bool {
        let mut trying = self.deleted.trying.lock().expect("conversations being deleted");
        match trying.get(key) {
            Some(None) => false,
            Some(Some(failed)) if failed.elapsed() < AGAIN => false,
            _ => {
                trying.insert(key.to_string(), None);
                true
            }
        }
    }

    /// The try to delete the scopes of the conversation `key` ended, all of
    /// them gone or not.
    fn tried_conversation(&self, key: &str, gone: bool) {
        let mut trying = self.deleted.trying.lock().expect("conversations being deleted");
        if gone {
            trying.remove(key);
        } else {
            trying.insert(key.to_string(), Some(std::time::Instant::now()));
        }
    }

    /// The owner library of `owner` has something of an ended period in
    /// it: it lets go of that at the next turn (`let_go`).
    fn lets_go(&self, owner: &Owner) {
        self.deleted.purging.lock().expect("libraries letting go").insert(owner.clone());
    }

    /// An owner library opens: what a scope deleted before the hub stopped
    /// left in it goes on going, and one deleted whose library was not
    /// reached then is let go of now.
    pub(super) fn library_opened(&self, owner: &Owner, library: &fastctx_content::v2::Library) {
        if self.look_through(owner, library).is_err() {
            self.deleted.unlooked.lock().expect("libraries to look through").insert(owner.clone());
        }
    }

    /// Lets the library of `owner` go of each scope deleted that it still
    /// holds, and notes it has something to let go of.
    fn look_through(&self, owner: &Owner, library: &fastctx_content::v2::Library) -> io::Result<()> {
        if library.purging().map_err(other)? {
            self.lets_go(owner);
        }
        if self.deleted_keys()?.lock().expect("deleted scopes").is_empty() {
            return Ok(());
        }
        for key in library.keys_ending("").map_err(other)? {
            if self.was_deleted(&key) && library.holding(&key).map_err(other)?.is_some() {
                library.revoke(&key).map_err(other)?;
                self.lets_go(owner);
            }
        }
        Ok(())
    }

    /// Deletes the scope `key` for good, described or not, so a description
    /// of it that comes later is refused (`crate::purges`): marked first, so
    /// one crossing this is turned away as it is written; then its row, if
    /// one was written before, deleted as any scope is, its holders told; or,
    /// with none, its mark kept for good. Call on a thread that may block.
    pub(crate) fn purge_key(&self, key: &str) -> io::Result<()> {
        if let Some(why) = self.sealed() {
            return Err(io::Error::other(why));
        }
        let keys = self.deleted_keys()?;
        let stamp = stamp(key);
        keys.lock().expect("deleted scopes").insert(stamp);
        // In one write, after any description that passed its look at the
        // marks before they had this one: its row, as written, deleted with
        // what the hub's history kept of it, and the mark kept for good,
        // once (codex-87c8bbd9).
        let (name, sealing) = (key.to_string(), self.sealing.clone());
        let written = self.control.write_now(1024, move |c| {
            use fastctx_storage::rusqlite::OptionalExtension;
            super::unsealed(&sealing)?;
            let owner: Option<String> = c
                .prepare_cached("SELECT owner FROM asset_scopes WHERE key = ?1")?
                .query_row([&name], |row| row.get(0))
                .optional()?;
            if let Some(owner) = &owner {
                c.prepare_cached("DELETE FROM asset_scopes WHERE key = ?1")?.execute([&name])?;
                for table in ["hub_checkpoints", "hub_epoch_facts", "hub_gap_facts"] {
                    c.prepare_cached(&format!("DELETE FROM {table} WHERE owner = ?1 AND scope = ?2"))?
                        .execute(params![owner, name])?;
                }
            } else if c
                .prepare_cached("SELECT 1 FROM hub_chain WHERE kind = ?1 AND digest = ?2")?
                .exists(params![KIND, stamp.to_vec()])?
            {
                return Ok((None, None));
            }
            Ok((owner, Some(chain::append(c, KIND, &stamp)?)))
        });
        // Not written: it stays marked, as one marked and not yet kept, and
        // is tried again as a whole.
        let (owner, appended) = written.map_err(io::Error::other)?;
        let witnessed = match appended {
            Some((seq, head)) => self.witness.advanced(seq, &head),
            None => Ok(()),
        };
        if let Some(owner) = owner {
            match Owner::from_key(&owner) {
                Some(owner) => self.let_go_of_stored(key, &owner),
                None => return Err(io::Error::other(format!("a scope of an unknown owner {owner}"))),
            }
        }
        witnessed
    }

    /// Deletes the scope `key`; whether there was one. Its machines are
    /// told by the pushes this leaves to be sent. Call on a thread that may
    /// block.
    pub(crate) fn delete_scope(&self, key: &str) -> io::Result<bool> {
        if let Some(why) = self.sealed() {
            return Err(io::Error::other(why));
        }
        let Some(scope) = scopes::scope(self, key).map_err(io::Error::other)? else { return Ok(false) };
        self.delete_stored(key, scope)
    }

    /// `delete_scope` of the scope `key` as read, `scope`.
    fn delete_stored(&self, key: &str, scope: scopes::Scope) -> io::Result<bool> {
        let keys = self.deleted_keys()?;
        let stamp = stamp(key);
        // Known as deleted before its row goes: what asks of it meanwhile is
        // answered as of one read no more, not as of one never described.
        keys.lock().expect("deleted scopes").insert(stamp);
        let (owner, name, sealing) = (scope.owner.key(), key.to_string(), self.sealing.clone());
        let written = self.control.write_now(1024, move |c| {
            super::unsealed(&sealing)?;
            if c.prepare_cached("DELETE FROM asset_scopes WHERE key = ?1")?.execute([&name])? == 0 {
                return Ok(None);
            }
            // What the hub's history kept of it goes with its row: an owner
            // library is held to what is kept there as it opens.
            for table in ["hub_checkpoints", "hub_epoch_facts", "hub_gap_facts"] {
                c.prepare_cached(&format!("DELETE FROM {table} WHERE owner = ?1 AND scope = ?2"))?
                    .execute(params![owner, name])?;
            }
            Ok(Some(chain::append(c, KIND, &stamp)?))
        });
        let witnessed = match written {
            Ok(Some((seq, head))) => self.witness.advanced(seq, &head),
            Ok(None) => return Ok(false),
            Err(error) => {
                // A write that failed may have been kept all the same: the
                // scope stays marked unless its row is seen to be there
                // still, and one whose row is gone is deleted.
                let name = key.to_string();
                let kept = self.control.read_now(move |c| {
                    Ok(c.prepare_cached("SELECT 1 FROM asset_scopes WHERE key = ?1")?.exists([&name])?)
                });
                match kept {
                    Ok(true) => {
                        keys.lock().expect("deleted scopes").remove(&stamp);
                        return Err(io::Error::other(error));
                    }
                    Ok(false) => self.catch_up(),
                    Err(_) => return Err(io::Error::other(error)),
                }
            }
        };
        self.let_go_of_stored(key, &scope.owner);
        witnessed?;
        Ok(true)
    }

    /// What follows the row of the scope `key` of `owner` deleted: its
    /// holders told it is revoked, and its owner library let go of it.
    fn let_go_of_stored(&self, key: &str, owner: &Owner) {
        if let Some(scopes) = self.changed.lock().expect("changed scopes").get_mut(owner) {
            scopes.remove(key);
        }
        self.receipted.lock().expect("receipted").remove(key);
        let held = self.holders.lock().expect("holders").remove(key).unwrap_or_default();
        self.pushes
            .lock()
            .expect("pushes")
            .extend(held.into_iter().map(|machine| (machine, Push::Revoked { scope: key.to_string() })));
        // The owner library holds it no more, and lets go of it in turns;
        // one not reached now is reached at a later turn.
        match self.library(owner).and_then(|library| library.revoke(key).map_err(other)) {
            Ok(()) => self.lets_go(owner),
            Err(_) => self.deleted.unreached.lock().expect("scopes unreached").push((owner.clone(), key.to_string())),
        }
    }

    /// A turn of the owner libraries at letting go of the scopes deleted
    /// from them: some passes each, the rest at the next turn; one turn at
    /// a time, so many conversations deleted together make one line of
    /// passes, not one each. Call on a thread that may block.
    pub(crate) fn let_go(&self) {
        let Ok(_turn) = self.deleted.turn.try_lock() else { return };
        let begun = std::time::Instant::now();
        let unlooked = std::mem::take(&mut *self.deleted.unlooked.lock().expect("libraries to look through"));
        for owner in unlooked {
            let looked = self.library(&owner).and_then(|library| self.look_through(&owner, &library));
            if looked.is_err() && !self.deleting(&owner).unwrap_or(false) {
                self.deleted.unlooked.lock().expect("libraries to look through").insert(owner);
            }
        }
        let unreached = std::mem::take(&mut *self.deleted.unreached.lock().expect("scopes unreached"));
        for (owner, key) in unreached {
            match self.library(&owner).and_then(|library| library.revoke(&key).map_err(other)) {
                Ok(()) => self.lets_go(&owner),
                // A library being deleted goes whole.
                Err(_) if self.deleting(&owner).unwrap_or(false) => {}
                Err(_) => self.deleted.unreached.lock().expect("scopes unreached").push((owner, key)),
            }
        }
        let owners: Vec<Owner> = self.deleted.purging.lock().expect("libraries letting go").iter().cloned().collect();
        for owner in owners {
            if begun.elapsed() >= TURN {
                break;
            }
            let done = (|| -> io::Result<bool> {
                let library = self.library(&owner)?;
                for _ in 0..PASSES_AT_ONCE {
                    if !library.purging().map_err(other)? {
                        return Ok(true);
                    }
                    if begun.elapsed() >= TURN {
                        return Ok(false);
                    }
                    library.purge_at_most(ROWS_A_PASS, SCOPES_A_PASS).map_err(other)?;
                    std::thread::sleep(PACE);
                }
                library.purging().map(|purging| !purging).map_err(other)
            })();
            // A library being deleted goes whole.
            if matches!(done, Ok(true)) || self.deleting(&owner).unwrap_or(false) {
                self.deleted.purging.lock().expect("libraries letting go").remove(&owner);
                // A scope deleted from it meanwhile put it here again, which
                // the line above undid: looked at once more.
                if self.library(&owner).is_ok_and(|library| library.purging().unwrap_or(false)) {
                    self.lets_go(&owner);
                }
            }
        }
    }
}

/// The scopes of the conversation at `conversation`: those ending with its
/// id whose conversation lived on its machine — born there, made there for
/// an account it assisted, or opened in the body that machine is. Ids are
/// unique on their machine alone, so another machine's conversation of the
/// same id is none of these.
pub(super) fn of_conversation(
    assets: &Assets,
    directory: &fastctx_net::directory_index::Indexed<'_>,
    conversation: &Address,
) -> Result<Vec<String>, String> {
    let ending = format!("/{}", conversation.resident);
    let keys: Vec<String> = assets
        .control
        .read_now(move |c| {
            let mut statement = c.prepare_cached("SELECT key FROM asset_scopes WHERE substr(key, -?2) = ?1 ORDER BY key")?;
            let rows = statement.query_map(params![ending, ending.len() as i64], |row| row.get(0))?;
            Ok(rows.collect::<std::result::Result<_, _>>()?)
        })
        .map_err(|error| error.to_string())?;
    let mut found = Vec::new();
    for key in keys {
        if scopes::machines_own(&key) {
            continue;
        }
        let Some(scope) = scopes::scope(assets, &key)? else { continue };
        let same = |machine: &MachineName| directory.same(machine, &conversation.machine);
        let lived = match &scope.descriptor.readers {
            Readers::Conversation { born_on: Some(machine), .. } => same(machine),
            // Made by the machine that described it, kept as its key and its
            // owner as opened: the same machine by its key, whoever owns it
            // now.
            Readers::Conversation { born_on: None, .. } => {
                let by_key = |identity: &str| identity.split(':').next().map(str::to_string);
                let now = scopes::machine_now(directory, &conversation.machine);
                match (scope.source.as_deref().and_then(by_key), now.as_deref().and_then(by_key)) {
                    (Some(made), Some(machine)) => made == machine,
                    _ => false,
                }
            }
            Readers::Body { body, .. } => {
                assets.bodies.machine(body).map_err(|error| error.to_string())?.is_some_and(|machine| same(&machine))
            }
            _ => false,
        };
        if lived {
            found.push(key);
        }
    }
    Ok(found)
}

/// The conversation at `conversation` was deleted where it lived: its
/// scopes are deleted, off the caller's thread, and the machines holding
/// them told. Asked again of one whose scopes are gone, nothing is done.
/// Once every one of them is gone, the deletion is told so
/// (`forget::scopes_went`); one that could not be deleted now is tried
/// again as the deletion is looked at, a minute apart at least.
pub(crate) fn conversation_gone(hub: &crate::Hub, conversation: &Address) {
    let Some(hub) = hub.this.get().and_then(std::sync::Weak::upgrade) else { return };
    let conversation = conversation.clone();
    let key = conversation.key();
    if !hub.assets.try_conversation(&key) {
        return;
    }
    // It changes the records: a hub moving waits for it (`moving`).
    let held = crate::moving::Moving::hold(&hub);
    fastctx_platform::task::spawn_blocking(move || {
        let _held = held;
        let gone = delete_its_scopes(&hub, &conversation);
        hub.assets.tried_conversation(&key, gone);
        if gone {
            crate::forget::scopes_went(&hub, &conversation);
        }
    });
}

/// Deletes the scopes of the conversation at `conversation`, purged where it
/// lived (`crate::purges`), at once: its own for good, described or not,
/// so one on its way is refused after (`Assets::purge_key`), and every other
/// it has. An error where not every one went, to be tried again.
pub(crate) fn purge_scopes(hub: &crate::Hub, conversation: &Address, its_own: &str) -> Result<(), String> {
    hub.assets.purge_key(its_own).map_err(|error| error.to_string())?;
    if !delete_its_scopes(hub, conversation) {
        return Err("not every scope of it could be deleted now".into());
    }
    Ok(())
}

/// Deletes the scopes of the conversation at `conversation`, deleted where
/// it lived, and tells the machines holding them; whether none is left.
fn delete_its_scopes(hub: &crate::Hub, conversation: &Address) -> bool {
    let keys = {
        let store = hub.store();
        of_conversation(&hub.assets, &store.indexed(), conversation)
    };
    let keys = match keys {
        Ok(keys) => keys,
        Err(error) => {
            hub.log.line(format_args!("finding the scopes of {}, deleted: {error}", conversation.key()));
            return false;
        }
    };
    if keys.is_empty() {
        return true;
    }
    let mut gone = true;
    for key in &keys {
        match hub.assets.delete_scope(key) {
            Ok(true) => hub.log.line(format_args!("deleted the scope {key} with its conversation")),
            Ok(false) => {}
            Err(error) => {
                gone = false;
                hub.log.line(format_args!("deleting the scope {key} with its conversation: {error}"));
            }
        }
    }
    let drained = hub.assets.drain_pushes();
    let pushes = {
        let store = hub.store();
        hub.assets.take_pushes(drained, &store.indexed())
    };
    for (machine, push) in pushes {
        hub.tell(&machine, fastctx_net::wire::Down::Assets { push });
    }
    hub.assets.let_go();
    gone
}
