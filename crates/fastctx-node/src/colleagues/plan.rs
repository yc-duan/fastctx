//! Plan selection shared with the account-plan command and context producer.

use fastctx_types::{
    Address,
    collaboration::PlanAccess,
    residents::{About, Origin},
};

/// A colleague reads its requester's plan and has no writable plan of its own.
/// The plan service must retain the conversation's acting account when fetching
/// this owner; the owner address does not grant access to another account.
pub(crate) fn access(about: &About) -> PlanAccess {
    match &about.origin {
        Origin::Colleague { asked_by, .. } => PlanAccess { owner: asked_by.clone(), read_only: true },
        Origin::User => PlanAccess {
            owner: Address { machine: about.machine.clone(), resident: about.id.clone() },
            read_only: false,
        },
    }
}
