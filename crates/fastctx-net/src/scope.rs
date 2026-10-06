//! What each member may do: its scope, and the grants that set it.
//!
//! A few machines the user trusts most hold management rights; the founder
//! is born with them. Only their grants count: a grant names a member by
//! name and key and says what it may use (which machines, which agents on
//! them, which projects), which of the World's actions it may take there,
//! whether it may invite others, and whether it manages too. A member that
//! has never been granted anything keeps the scope its invitation gave it,
//! or, for the members of a World older than scopes, everything.
//!
//! Every machine works out every member's scope for itself, from the signed
//! records the hub hands out (`crate::trust`), and refuses at its own door
//! whatever a caller's scope does not cover.

use fastctx_types::MachineName;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

pub use fastctx_types::scope::{Action, Denied, Scope, Set};

use crate::sign::PublicKey;
use crate::trust::WorldId;

/// A management machine's word on what `member` may do, signed by it.
/// Each grant for a member carries the next version; a machine takes the
/// latest one it can trust, in the order the grants were made.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grant {
    pub world: WorldId,
    pub member: MachineName,
    /// The member's key, so a grant never carries over to another machine
    /// that took the name later.
    pub key: PublicKey,
    pub scope: Scope,
    /// Whether the member holds management rights from now on.
    pub manage: bool,
    pub version: u64,
    pub by: MachineName,
    pub at: Timestamp,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sets_read_and_combine_as_written() {
        let only: Set<MachineName> = serde_json::from_str(r#"["desktop","laptop"]"#).unwrap();
        let all: Set<MachineName> = serde_json::from_str(r#""all""#).unwrap();
        assert!(all.is_all());
        assert_eq!(serde_json::to_string(&only).unwrap(), r#"["desktop","laptop"]"#);
        let one: Set<MachineName> = serde_json::from_str(r#"["laptop"]"#).unwrap();
        assert!(one.within(&only) && only.within(&all) && !all.within(&only));
        assert_eq!(only.intersect(&all), only);
        assert_eq!(only.intersect(&one), one);
        assert!(serde_json::from_str::<Set<MachineName>>(r#""some""#).is_err());
    }
}
