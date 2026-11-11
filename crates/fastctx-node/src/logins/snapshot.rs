//! The persisted projection and the bounded work that refreshes it.
mod build;
mod row;
mod state;

pub(super) use state::{Listing, ask, chosen, kept, part, progress, quota, read, refresh, register};
