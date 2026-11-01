//! Plan selection shared with the account-plan command and context producer.

use fastctx_types::{Address, collaboration::PlanAccess, residents::About};

/// Every conversation, a colleague as much as the one that asked it, keeps a
/// plan of its own and writes it. The plan service must retain the
/// conversation's acting account when fetching this owner; the owner address
/// does not grant access to another account.
pub(crate) fn access(about: &About) -> PlanAccess {
    PlanAccess { owner: Address { machine: about.machine.clone(), resident: about.id.clone() }, read_only: false }
}
