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
use std::sync::{Mutex, OnceLock};

/// What the chain calls a scope's deletion.
const KIND: &str = "scope-deleted";
/// The passes an owner library is given at a time to let go of what it
/// kept, the rows of each kind a pass takes, and the pause between two: its
/// writer serves requests meanwhile, and a frame's body goes with its row.
const PASSES_AT_ONCE: usize = 64;
const ROWS_A_PASS: i64 = 16;
const PACE: std::time::Duration = std::time::Duration::from_millis(20);
/// The longest a turn of all the libraries together takes.
const TURN: std::time::Duration = std::time::Duration::from_secs(2);

/// The scopes deleted with their conversations, and the owner libraries
/// still letting go of them.
#[derive(Default)]
pub(crate) struct Deleted {
    /// What the chain keeps of each, read from it once.
    keys: OnceLock<Mutex<HashSet<[u8; 32]>>>,
    purging: Mutex<BTreeSet<Owner>>,
    /// Those whose owner library could not be reached as they were deleted:
    /// tried again at each turn (`let_go`).
    unreached: Mutex<Vec<(Owner, String)>>,
}

/// What the chain keeps of the deletion of the scope `key`.
fn stamp(key: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"fastctx-hub-scope-deleted/1");
    hasher.update(key.as_bytes());
    hasher.finalize().into()
}

fn other(error: impl std::fmt::Display) -> io::Error {
    io::Error::other(error.to_string())
}

impl Assets {
    fn deleted_keys(&self) -> io::Result<&Mutex<HashSet<[u8; 32]>>> {
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
        Ok(self.deleted.keys.get_or_init(|| Mutex::new(kept)))
    }

    /// Whether the scope `key` was deleted with its conversation.
    pub(crate) fn was_deleted(&self, key: &str) -> bool {
        self.deleted_keys().is_ok_and(|keys| {
            let keys = keys.lock().expect("deleted scopes");
            !keys.is_empty() && keys.contains(&stamp(key))
        })
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
        if library.purging().unwrap_or(false) {
            self.lets_go(owner);
        }
        if !self.deleted_keys().is_ok_and(|keys| !keys.lock().expect("deleted scopes").is_empty()) {
            return;
        }
        let Ok(keys) = library.keys_ending("") else { return };
        for key in keys {
            if self.was_deleted(&key)
                && library.holding(&key).ok().flatten().is_some()
                && library.revoke(&key).is_ok()
            {
                self.lets_go(owner);
            }
        }
    }

    /// Deletes the scope `key`; whether there was one. Its machines are
    /// told by the pushes this leaves to be sent. Call on a thread that may
    /// block.
    pub(crate) fn delete_scope(&self, key: &str) -> io::Result<bool> {
        if let Some(why) = self.sealed() {
            return Err(io::Error::other(why));
        }
        let Some(scope) = scopes::scope(self, key).map_err(io::Error::other)? else { return Ok(false) };
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
        let (seq, head) = match written {
            Ok(Some(appended)) => appended,
            Ok(None) => return Ok(false),
            Err(error) => {
                keys.lock().expect("deleted scopes").remove(&stamp);
                return Err(io::Error::other(error));
            }
        };
        let witnessed = self.witness.advanced(seq, &head);
        if let Some(scopes) = self.changed.lock().expect("changed scopes").get_mut(&scope.owner) {
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
        match self.library(&scope.owner).and_then(|library| library.revoke(key).map_err(other)) {
            Ok(()) => self.lets_go(&scope.owner),
            Err(_) => self.deleted.unreached.lock().expect("scopes unreached").push((scope.owner.clone(), key.to_string())),
        }
        witnessed?;
        Ok(true)
    }

    /// A turn of the owner libraries at letting go of the scopes deleted
    /// from them: some passes each, the rest at the next turn. Call on a
    /// thread that may block.
    pub(crate) fn let_go(&self) {
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
        let begun = std::time::Instant::now();
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
                    library.purge_at_most(ROWS_A_PASS).map_err(other)?;
                    std::thread::sleep(PACE);
                }
                library.purging().map(|purging| !purging).map_err(other)
            })();
            // A library being deleted goes whole.
            if matches!(done, Ok(true)) || self.deleting(&owner).unwrap_or(false) {
                self.deleted.purging.lock().expect("libraries letting go").remove(&owner);
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
    directory: &fastctx_net::directory::Directory,
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
        let lived: Option<MachineName> = match &scope.descriptor.readers {
            Readers::Conversation { born_on: Some(machine), .. } => Some(machine.clone()),
            Readers::Conversation { born_on: None, .. } => {
                scope.source.clone().and_then(|machine| MachineName::try_from(machine).ok())
            }
            Readers::Body { body, .. } => assets.bodies.machine(body).map_err(|error| error.to_string())?,
            _ => None,
        };
        if lived.is_some_and(|machine| directory.same(&machine, &conversation.machine)) {
            found.push(key);
        }
    }
    Ok(found)
}

/// The conversation at `conversation` was deleted where it lived: its
/// scopes are deleted, off the caller's thread, and the machines holding
/// them told. Asked again of one whose scopes are gone, nothing is done.
pub(crate) fn conversation_gone(hub: &crate::Hub, conversation: &Address) {
    let Some(hub) = hub.this.get().and_then(std::sync::Weak::upgrade) else { return };
    let conversation = conversation.clone();
    fastctx_platform::task::spawn_blocking(move || {
        let keys = {
            let store = hub.store();
            of_conversation(&hub.assets, &store.book.directory, &conversation)
        };
        let keys = match keys {
            Ok(keys) => keys,
            Err(error) => {
                hub.log.line(format_args!("finding the scopes of {}, deleted: {error}", conversation.key()));
                return;
            }
        };
        if keys.is_empty() {
            return;
        }
        for key in &keys {
            match hub.assets.delete_scope(key) {
                Ok(true) => hub.log.line(format_args!("deleted the scope {key} with its conversation")),
                Ok(false) => {}
                Err(error) => hub.log.line(format_args!("deleting the scope {key} with its conversation: {error}")),
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
    });
}
