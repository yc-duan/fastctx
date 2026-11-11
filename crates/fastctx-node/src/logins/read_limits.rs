//! Bounds for external configuration inputs used by login background work.
//!
//! The bounds are there against a file that is no configuration at all, and
//! stand well past what an agent's own files grow to in use: Claude Code
//! keeps every project's history in its `.claude.json`, megabytes of it on a
//! machine long in use, and a login whose file is past a bound reads as one
//! whose identity failed.
use fastctx_platform::read_budget::Limits;
pub(super) const FILE_BYTES: usize = 64 * 1024 * 1024;
pub(super) const TOTAL_BYTES: usize = 256 * 1024 * 1024;
pub(super) const FILES: usize = 4096;
pub(super) const PROJECTION: Limits = Limits { file: FILE_BYTES, total: TOTAL_BYTES, files: FILES };
pub(super) const IDENTITY: Limits = Limits { file: FILE_BYTES, total: 128 * 1024 * 1024, files: 64 };
pub(super) const DIGEST: Limits = Limits { file: FILE_BYTES, total: 128 * 1024 * 1024, files: 8 };
