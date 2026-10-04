//! The World's hands: `run`, the shell; `view`, for looking at files; and
//! the writing core behind `edit`, `write` and `apply_patch`.

pub mod run;

pub mod encoding;
pub mod files;
pub mod write;
