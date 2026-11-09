//! The hub's long-term service signing key. It is created once, when the hub
//! first runs this version, and pinned in the hub's own database; afterwards a
//! missing key file is a damaged identity, never a reason to make another.
use std::{path::Path, sync::Arc};

use fastctx_net::{sign::SecretKey, sync_proof::IdentitySigner};
use fastctx_sync::{
    blocks::SqliteBlocks,
    cursor::Revision,
    identity::HubId,
    metrics::PlatformMetrics,
    proof::{HubSigningIdentity, ProofSigner, SigningKey},
    store::{Error, control},
};

const FILE: &str = "signing.key";

pub(super) async fn load(
    dir: &Path,
    hub: &HubId,
    blocks: &SqliteBlocks,
) -> Result<(Arc<dyn ProofSigner>, HubSigningIdentity), Error> {
    let path = dir.join("sync").join(FILE);
    let pinned = control::pinned_key(blocks, hub).await?;
    let read = path.clone();
    let stored = fastctx_platform::task::spawn_blocking(move || match fastctx_platform::disk::read(&read) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    })
    .await
    .map_err(|error| Error::Unavailable(error.to_string()))?
    .map_err(|error| Error::Unavailable(format!("reading the hub's signing key: {error}")))?;
    let key = match (stored, &pinned) {
        (Some(bytes), _) => {
            let seed: [u8; 32] =
                bytes.try_into().map_err(|_| Error::Corrupt("the hub's signing key file is damaged".into()))?;
            SecretKey::from_seed(&seed)
        }
        (None, Some(_)) => {
            return Err(Error::Corrupt("the hub's signing key is missing although its public key is pinned".into()));
        }
        (None, None) => {
            let key = SecretKey::generate().map_err(|error| Error::Unavailable(error.to_string()))?;
            let seed = key.seed().to_vec();
            fastctx_platform::task::spawn_blocking(move || {
                fastctx_platform::ensure_private_dir(path.parent().expect("a key file has a directory"))?;
                fastctx_platform::atomic::publish_new(&path, &seed, fastctx_platform::atomic::Permissions::Private)
            })
            .await
            .map_err(|error| Error::Unavailable(error.to_string()))?
            .map_err(|error| Error::Unavailable(format!("writing the hub's signing key: {error}")))?;
            key
        }
    };
    let identity = HubSigningIdentity {
        hub: hub.clone(),
        key: SigningKey(*key.public().as_bytes()),
        revision: Revision(1),
        rotations: Vec::new(),
    };
    match &pinned {
        Some(pinned) if pinned.key != identity.key => {
            return Err(Error::Corrupt("the hub's signing key differs from the key it pinned".into()));
        }
        Some(_) => {}
        None => control::pin_key(blocks, &identity).await?,
    }
    let signer = IdentitySigner::hub(hub.clone(), Arc::new(key), Arc::new(PlatformMetrics));
    Ok((Arc::new(signer), identity))
}
