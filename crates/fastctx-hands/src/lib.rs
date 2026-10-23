//! The World's hands: `run`, the shell; `view`, for looking at files; the
//! writing core behind `edit`, `write` and `apply_patch`; and how what the
//! World tells a resident reads.

pub mod answer;
pub mod copy;
pub mod encoding;
pub mod files;
pub mod folders;
pub mod image;
pub mod lines;
pub mod memory;
pub mod memory_path;
pub mod outline;
pub mod pages;
pub mod run;
mod spool;
mod text;
pub mod told;
pub mod town;
pub mod view;
pub mod write;
