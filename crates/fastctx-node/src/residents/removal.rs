//! Removal is recorded in the conversation's own directory and survives
//! host reconnects. Nothing moves: running jobs keep the paths they hold, and
//! the records stay where they are until the user deletes the conversation.
//!
//! A deletion first marks the resident as being deleted, in memory: from
//! then on it takes nothing new, while its turn and commands are stopped.
//! When its records go, a tombstone stays (`tombstones`).

use std::io;

use fastctx_platform::{Permissions, publish_new};
use fastctx_types::ResidentId;
use fastctx_types::ipc::Failure;
use fastctx_types::layout::WorldDir;
use jiff::Timestamp;

use super::Residents;

pub fn removal(world: &WorldDir, id: &ResidentId) -> io::Result<Option<Timestamp>> {
    super::records::read_removed(&world.conversation(id))
}

pub(super) fn refused(id: &ResidentId) -> Failure {
    Failure::not_run(format!(
        "{id} was removed from the World; this session can no longer use World tools or be woken; open a new session to become a resident"
    ))
}

/// Held while a deletion of a resident is under way.
pub struct Deleting<'a> {
    residents: &'a Residents,
    id: ResidentId,
}

impl Drop for Deleting<'_> {
    fn drop(&mut self) {
        let mut state = self.residents.lock();
        if let Some(count) = state.deleting.get_mut(&self.id) {
            *count -= 1;
            if *count == 0 {
                state.deleting.remove(&self.id);
            }
        }
    }
}

impl Residents {
    /// Whether the resident `id` takes something new: words, a turn, a
    /// command, a call of its session. What it refuses was not done.
    pub fn active(&self, id: &ResidentId) -> Result<(), Failure> {
        self.stoppable(id)?;
        if self.lock().deleting.contains_key(id) {
            return Err(Failure::not_run(format!("{id} is being deleted")));
        }
        Ok(())
    }

    /// Whether the turn of the resident `id` can be stopped: it lives here
    /// and is not removed, though it may be being deleted.
    pub fn stoppable(&self, id: &ResidentId) -> Result<(), Failure> {
        let mut state = self.lock();
        let Some(live) = state.live.get(id) else {
            return Err(Failure::not_run(match state.tombstones.of(id) {
                Some(_) => format!("{id} was deleted"),
                None => format!("no resident {id} lives here"),
            }));
        };
        if live.removed.is_some() { Err(refused(id)) } else { Ok(()) }
    }

    /// Marks `id` as being deleted until what is returned is dropped.
    pub fn deleting(&self, id: &ResidentId) -> Deleting<'_> {
        *self.lock().deleting.entry(id.clone()).or_default() += 1;
        Deleting { residents: self, id: id.clone() }
    }

    pub fn is_deleting(&self, id: &ResidentId) -> bool {
        self.lock().deleting.contains_key(id)
    }

    /// When the conversation of `id` was deleted from this machine, if it
    /// was lately.
    pub fn deleted(&self, id: &ResidentId) -> Option<Timestamp> {
        self.lock().tombstones.of(id).map(|tombstone| tombstone.at)
    }

    /// The deleted conversation the host session `session` was, if any.
    pub fn deleted_session(&self, session: &str) -> Option<ResidentId> {
        self.lock().tombstones.holding(session).cloned()
    }

    /// Every conversation deleted from this machine lately.
    pub fn tombstones(&self) -> Vec<(ResidentId, super::Tombstone)> {
        self.lock().tombstones.all().map(|(id, tombstone)| (id.clone(), tombstone.clone())).collect()
    }

    pub fn removed(&self, id: &ResidentId) -> Option<Timestamp> {
        self.lock().live.get(id).and_then(|live| live.removed)
    }

    pub fn management(
        &self,
        id: &ResidentId,
    ) -> Result<crate::leases::Lease<ResidentId, fastctx_platform::doing::RwLock<()>>, Failure> {
        self.lock()
            .live
            .contains_key(id)
            .then(|| self.management.get_or_insert_with(id, || crate::ranked::management(id)))
            .ok_or_else(|| Failure::not_run(format!("no resident {id} lives here")))
    }

    /// Records that the user removed `id`; a reconnect can never turn it
    /// back into an arrival. Returns its directory, where its records stay.
    pub fn remove(&self, world: &WorldDir, id: &ResidentId) -> io::Result<String> {
        let mut state = self.lock();
        let _transaction = crate::resident_storage::lock();
        let live = state.live.get_mut(id).ok_or_else(|| io::Error::other("unknown resident"))?;
        let directory = world.conversation(id);
        if live.removed.is_none() {
            let at = Timestamp::now();
            match publish_new(
                &directory.removed(),
                &serde_json::to_vec(&at).map_err(io::Error::other)?,
                Permissions::Private,
            ) {
                Ok(()) => live.removed = Some(at),
                // A removal that was recorded before the node last stopped.
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    live.removed = super::records::read_removed(&directory)?;
                }
                Err(error) => return Err(error),
            }
        }
        Ok(crate::world::native(directory.path()))
    }

    /// Brings back a resident the user archived: it is listed again, its
    /// session may use World tools and be woken again.
    pub fn restore(&self, world: &WorldDir, id: &ResidentId) -> io::Result<()> {
        let mut state = self.lock();
        let _transaction = crate::resident_storage::lock();
        let live = state.live.get_mut(id).ok_or_else(|| io::Error::other("unknown resident"))?;
        match std::fs::remove_file(world.conversation(id).removed()) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
            _ => {}
        }
        live.removed = None;
        Ok(())
    }

    /// Forgets a resident whose conversation is being deleted from this
    /// machine: it is no longer on record, and a tombstone says it was, with
    /// the host sessions it had (`sessions`), before anything is forgotten.
    pub fn forget(&self, world: &WorldDir, id: &ResidentId, mut sessions: Vec<String>) -> io::Result<()> {
        let mut state = self.lock();
        let Some(live) = state.live.get(id) else { return Ok(()) };
        let session = live.about.session.clone();
        let inbox = live.inbox.clone();
        let asked_by = match &live.about.origin {
            fastctx_types::residents::Origin::Colleague { asked_by, .. } => Some(asked_by.clone()),
            fastctx_types::residents::Origin::User => None,
        };
        if let Some(session) = &session
            && !sessions.contains(session)
        {
            sessions.push(session.clone());
        }
        state.tombstones.bury(world, id, super::Tombstone { at: Timestamp::now(), sessions })?;
        if let Some(session) = &session {
            if state.threads.get::<ResidentId>(session).as_ref() == Some(id) {
                state.threads.remove(session);
            }
            state.forget_session(session);
        }
        if let Some(inbox) = inbox
            && state.inboxes.get::<ResidentId>(&inbox).as_ref() == Some(id)
        {
            state.inboxes.remove(&inbox);
        }
        if let Some(asked_by) = asked_by
            && let Some(started) = state.colleagues.get_mut(&asked_by)
        {
            started.retain(|colleague| colleague != id);
        }
        state.live.remove(id);
        Ok(())
    }
}
