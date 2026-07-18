//! Trusted startup update checks and crash-safe handoff to a copied updater process.

mod cache;
mod check;
mod helper;
mod model;

pub(crate) use check::{last_check_status, spawn_update_check};
pub(crate) use helper::{
    FinalizeNotice, FinalizeOutcome, NPM_LAUNCHER_WAIT_EXIT_CODE, UPDATE_FAILURE_ENV,
    UPDATE_FINALIZE_ENV, UpdateStart, begin_update, cleanup_replaced_binaries, finalize_update,
    run_update_helper,
};
pub(crate) use model::{CheckFailure, CheckFailureKind, StartupUpdate, UpdatePlan};
