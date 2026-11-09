//! Canonical production over source-gated keyed state and immutable body pages.
//! Planning never commits. Publish the plan in its issued step, then commit its
//! facts and effects together with the consumed input prefix's checkpoint.
mod alias_facts;
mod api;
mod append;
mod budget;
mod content;
mod control_facts;
mod dispatch;
mod emit;
mod item_facts;
mod jobs;
mod keys;
mod pending;
mod plan;
mod rows;
mod state;

pub use api::*;
pub use plan::plan_step;
