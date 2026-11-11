//! Direct paths remain available when late submission families cannot be served.

use crate::submitted::Family;
use fastctx_types::ipc::Failure;
use std::{collections::HashSet, sync::Mutex};

#[derive(Default)]
pub(crate) struct Started(Mutex<HashSet<&'static str>>);

impl Started {
    pub(crate) fn has(&self, family: &Family) -> bool {
        self.0.lock().expect("submission registrations").contains(family.name)
    }

    pub(crate) fn registered(&self, family: &Family) {
        self.0.lock().expect("submission registrations").insert(family.name);
    }
}

/// The submission engine could not record the asking, so nothing of it was
/// begun: the call is then done where it stands, as it was before its family
/// submitted. Accepted work, an unread answer and execution failures must
/// never trigger a second synchronous action.
pub(crate) fn unrecorded(family: &Family, failure: &Failure) -> bool {
    failure.not_run
        && failure.code.is_none()
        && failure.denial.is_none()
        && failure.capacity.is_none()
        && failure.message.starts_with(&format!("recording {}:", family.name))
}
