//! Pure, bounded projection of already parsed conversation facts.
//!
//! The caller supplies only keyed rows needed by one fact. It commits the
//! returned states, indexes and canonical revisions atomically with the input.
//! Missing dependencies stay durable and are retried when their keyed row
//! changes. Child relocation and call fan-out are paged by that caller, never
//! expanded into a full history here. No function opens a file, host or store.
//! The caller verifies operation replay against retained source facts; the
//! selected lanes are bounded state, not a replacement for that durable index.
//!
//! A PreparedPut deliberately has no invented content hash: the registered
//! writer realizes its descriptor and owned content closure, then calls finish
//! with that ContentRef. AppendPlan similarly reuses the old text tree and one
//! new chunk. Source evidence is retained even when its display lane loses.


mod alias;
mod control;
mod item;
mod model;
mod placement;
pub mod presentation;

pub use alias::{AliasReduction, bind_alias, redirect_observation};
pub use control::{AgentState, BranchReduction, RewriteReduction, SourceState, agent, branch, picture, rewrite, turn};
pub use item::{AppendOutcome, AppendPlan, append, observe, place, run};
pub use model::*;
