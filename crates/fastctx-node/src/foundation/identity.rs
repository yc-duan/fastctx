//! What the foundation signs and holds with: the World's device key, the same
//! for every hub, and the alone hub its own conversations live under.
use std::{io, sync::Arc};

use fastctx_net::sync_proof::IdentitySigner;
use fastctx_sync::{
    identity::{DeviceId, Digest, HubId, OperationId},
    metrics::PlatformMetrics,
    proof::ProofSigner,
};
use fastctx_types::layout::WorldDir;

pub(super) struct Identity {
    pub device: DeviceId,
    pub alone: HubId,
    pub signer: Arc<dyn ProofSigner>,
}
impl Identity {
    pub(super) fn load(world: &WorldDir) -> io::Result<Self> {
        let key = crate::ledger::own_key(world)?;
        let device = DeviceId::from_bytes(*key.public().as_bytes());
        let alone = crate::ledger::alone(world)?;
        let signer = Arc::new(IdentitySigner::device(alone.clone(), Arc::new(key), Arc::new(PlatformMetrics)));
        Ok(Self { device, alone, signer })
    }
}

/// The owner this process claims tasks and content work as. It differs on
/// every start, so a restarted node never takes its predecessor's leases for
/// its own; those expire and are claimed again.
pub(super) fn claims(device: &DeviceId) -> OperationId {
    let started =
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |elapsed| elapsed.as_nanos());
    let seed = format!("fastctx-node/foundation-claims/v1\0{}\0{}\0{started}", device.as_str(), std::process::id());
    OperationId::from_bytes(Digest::of(seed.as_bytes()).bytes())
}
