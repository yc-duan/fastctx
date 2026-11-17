//! The persisted projection and the bounded work that refreshes it.
mod build;
mod legacy;

mod row;
mod state;

pub(super) use state::{
    Listing, agents, ask, before_reading, chosen, kept, part, progress, quota, read, refresh, register, renew,
};
