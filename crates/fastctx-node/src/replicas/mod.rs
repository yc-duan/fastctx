//! Current plan and history copies on every eligible computer.

pub(crate) mod backend;
pub(crate) mod files;
pub(crate) mod location;
pub(crate) mod publication;
pub(crate) mod state;
pub(crate) mod sync;
pub(crate) mod updates;
pub(crate) mod writing;

#[derive(Default)]
pub(crate) struct Copies {
    pub state: state::State,
    pub updates: updates::Updates,
}
