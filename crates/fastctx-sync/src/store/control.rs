//! What a hub signs control statements under: the authority epoch it began this
//! run with, and one sequence numbering every control statement it signs.
use crate::{
    blocks::SqliteBlocks,
    cursor::{Revision, Sequence},
    identity::{AuthorityEpoch, HubId},
    proof::{HubSigningIdentity, SigningKey},
    store::{
        Error,
        boundary::{self, Role},
        sql,
    },
};
use fastctx_storage::{
    Result as SqlResult,
    rusqlite::{Connection, OptionalExtension, params},
};
use std::sync::Arc;

/// Random bytes from the service's own entropy source. Engines draw new
/// incarnations from it and never generate randomness themselves.
pub type Entropy = Arc<dyn Fn() -> std::io::Result<[u8; 32]> + Send + Sync>;

/// One authority run. Its epoch is persisted before the run accepts a write, so
/// a hub restored from a backup never reissues a version another holder saw.
#[derive(Clone)]
pub struct AuthorityRun {
    hub: HubId,
    epoch: AuthorityEpoch,
    entropy: Entropy,
}
impl AuthorityRun {
    pub async fn begin(blocks: &SqliteBlocks, hub: HubId, entropy: Entropy) -> Result<Self, Error> {
        let drawn = entropy().map_err(|error| Error::Unavailable(format!("no random source: {error}")))?;
        let mut bytes = [0; 16];
        bytes.copy_from_slice(&drawn[..16]);
        let epoch = AuthorityEpoch::from_bytes(bytes);
        let (key, value) = (hub.to_string(), epoch.bytes());
        blocks
            .database()
            .write(1024, move |db| {
                let _boundary = boundary::enter_typed_write(Role::Authority);
                db.prepare_cached("INSERT INTO sync_hub_control_heads(hub,sequence,authority_epoch) VALUES(?1,?2,?3) ON CONFLICT(hub) DO UPDATE SET authority_epoch=excluded.authority_epoch")?
                    .execute(params![key, 0_u64.to_be_bytes().as_slice(), value.as_slice()])?;
                Ok(())
            })
            .await
            .map_err(|error| sql::error(error, blocks.worker().clone()))?;
        Ok(Self { hub, epoch, entropy })
    }
    pub fn hub(&self) -> &HubId {
        &self.hub
    }
    pub fn epoch(&self) -> &AuthorityEpoch {
        &self.epoch
    }
    /// Fresh random bytes for a new incarnation, such as a feed or producer epoch.
    pub(crate) fn draw(&self) -> SqlResult<[u8; 32]> {
        (self.entropy)().map_err(|error| {
            fastctx_storage::Error::with_kind(
                fastctx_storage::ErrorKind::Unavailable,
                format!("no random source: {error}"),
            )
        })
    }
}

/// The next control sequence, allocated in the caller's transaction.
pub(crate) fn next_control(db: &Connection, hub: &HubId) -> SqlResult<Sequence> {
    let current = db
        .prepare_cached("SELECT sequence FROM sync_hub_control_heads WHERE hub=?1")?
        .query_row(params![hub.to_string()], |row| sql::counter(row, 0))?;
    let next = current.checked_add(1).ok_or_else(|| sql::invalid("the control sequence is exhausted"))?;
    db.prepare_cached("UPDATE sync_hub_control_heads SET sequence=?2 WHERE hub=?1")?
        .execute(params![hub.to_string(), next.to_be_bytes().as_slice()])?;
    Ok(Sequence(next))
}

/// Pins `identity`'s key for its hub. A hub records its own service key the
/// first time it starts; a device records the key its authenticated
/// connection advertised. Pinning the same key again is a no-op. A different
/// key replaces a pinned one only through a rotation the old key signed,
/// which this entry does not accept.
pub async fn pin_key(blocks: &SqliteBlocks, identity: &HubSigningIdentity) -> Result<(), Error> {
    let (hub, key, revision) = (identity.hub.to_string(), identity.key.0, identity.revision);
    blocks
        .database()
        .write(1024, move |db| {
            let _boundary = boundary::enter_typed_write(Role::Authority);
            let current: Option<Vec<u8>> = db
                .prepare_cached("SELECT public_key FROM sync_hub_signing_keys WHERE hub=?1 AND current=1")?
                .query_row(params![hub], |row| row.get(0))
                .optional()?;
            match current {
                Some(pinned) if pinned == key => Ok(()),
                Some(_) => Err(sql::invalid("another key is pinned for this hub; only its signed rotation replaces it")),
                None => {
                    db.prepare_cached("INSERT INTO sync_hub_signing_keys(hub,public_key,revision,rotation_proof,current) VALUES(?1,?2,?3,NULL,1)")?
                        .execute(params![hub, key.as_slice(), revision.0.to_be_bytes().as_slice()])?;
                    Ok(())
                }
            }
        })
        .await
        .map_err(|error| sql::error(error, blocks.worker().clone()))
}

/// The key pinned for `hub`, if any.
pub async fn pinned_key(blocks: &SqliteBlocks, hub: &HubId) -> Result<Option<HubSigningIdentity>, Error> {
    let name = hub.to_string();
    let found = blocks
        .database()
        .read(move |db| {
            db.prepare_cached("SELECT public_key,revision FROM sync_hub_signing_keys WHERE hub=?1 AND current=1")?
                .query_row(params![name], |row| Ok((row.get::<_, Vec<u8>>(0)?, sql::counter(row, 1)?)))
                .optional()
                .map_err(Into::into)
        })
        .await
        .map_err(|error| sql::error(error, blocks.worker().clone()))?;
    found
        .map(|(key, revision)| {
            let key: [u8; 32] =
                key.try_into().map_err(|_| Error::Corrupt("a pinned hub key has a wrong length".into()))?;
            Ok(HubSigningIdentity {
                hub: hub.clone(),
                key: SigningKey(key),
                revision: Revision(revision),
                rotations: Vec::new(),
            })
        })
        .transpose()
}
