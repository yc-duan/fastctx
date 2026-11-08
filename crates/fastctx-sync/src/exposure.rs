//! Sharing becomes effective only after durable physical cleanup confirmation.

use crate::{
    cursor::Revision,
    identity::{DeviceId, HubId},
    store::FutureResult,
};
pub use fastctx_types::sync::exposure::*;

/// The hub persists an intent before changing the active directory. Receipt
/// acceptance rechecks its current generation, plan digest and machine identity.
pub trait ExposureCoordinator: Send + Sync {
    fn prepare(&self, intent: ExposureIntent) -> FutureResult<'_, ShareActivation>;
    fn confirm(&self, receipt: SignedExposureReceipt) -> FutureResult<'_, ShareActivation>;
    fn state(&self, hub: HubId, machine: DeviceId) -> FutureResult<'_, Option<ShareActivation>>;
}

#[derive(Clone, Debug)]
pub enum CleanupProgress {
    Pending { generation: Revision, reason: Option<String> },
    Complete { receipt: SignedExposureReceipt },
}

/// Invalidates old publications first, then removes all disallowed physical
/// copies. A cleanup budget may yield, but cannot turn partial cleanup into ACK.
pub trait ExposureCleaner: Send + Sync {
    fn prepare(&self, intent: ExposureIntent) -> FutureResult<'_, CleanupProgress>;
    fn advance(&self, generation: Revision, max_entries: u32) -> FutureResult<'_, CleanupProgress>;
}
