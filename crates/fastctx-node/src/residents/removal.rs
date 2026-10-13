//! Removal is recorded in the conversation's own directory and survives
//! host reconnects. Nothing moves: running jobs keep the paths they hold, and
//! the records stay where they are until the user deletes the conversation.

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
    Failure::new(format!(
        "{id} was removed from the World; this session can no longer use World tools or be woken; open a new session to become a resident"
    ))
}

impl Residents {
    pub fn active(&self, id: &ResidentId) -> Result<(), Failure> {
        let mut state = self.lock();
        let live = state.live.get(id).ok_or_else(|| Failure::new(format!("no resident {id} lives here")))?;
        if live.removed.is_some() { Err(refused(id)) } else { Ok(()) }
    }

    pub fn removed(&self, id: &ResidentId) -> Option<Timestamp> {
        self.lock().live.get(id).and_then(|live| live.removed)
    }

    pub fn management(
        &self,
        id: &ResidentId,
    ) -> Result<crate::leases::Lease<ResidentId, tokio::sync::RwLock<()>>, Failure> {
        self.lock()
            .live
            .contains_key(id)
            .then(|| self.management.get_or_insert_with(id, Default::default))
            .ok_or_else(|| Failure::new(format!("no resident {id} lives here")))
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

    /// Forgets a resident whose conversation was deleted from this machine:
    /// it is no longer on record, and a session that comes back as it is a
    /// new one.
    pub fn forget(&self, id: &ResidentId) {
        let mut state = self.lock();
        let Some(live) = state.live.get(id) else { return };
        let session = live.about.session.clone();
        let inbox = live.inbox.clone();
        let asked_by = match &live.about.origin {
            fastctx_types::residents::Origin::Colleague { asked_by, .. } => Some(asked_by.clone()),
            fastctx_types::residents::Origin::User => None,
        };
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
    }
}
