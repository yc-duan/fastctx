//! Bounds for external configuration inputs used by login background work.
use fastctx_platform::read_budget::Limits;
pub(super) const FILE_BYTES: usize = 2 * 1024 * 1024;
pub(super) const TOTAL_BYTES: usize = 16 * 1024 * 1024;
pub(super) const FILES: usize = 4096;
pub(super) const PROJECTION: Limits = Limits { file: FILE_BYTES, total: TOTAL_BYTES, files: FILES };
pub(super) const IDENTITY: Limits = Limits { file: FILE_BYTES, total: 8 * 1024 * 1024, files: 16 };
pub(super) const DIGEST: Limits = Limits { file: FILE_BYTES, total: 4 * 1024 * 1024, files: 2 };
