//! What a hub signs control statements under: the authority epoch it began this
//! run with, and one sequence numbering every control statement it signs.
use crate::{
    blocks::SqliteBlocks,
    cursor::Sequence,
    identity::{AuthorityEpoch, HubId},
    store::{
        Error,
        boundary::{self, Role},
        sql,
    },
};
use fastctx_storage::{
    Result as SqlResult,
    rusqlite::{Connection, params},
};
use std::sync::Arc;

/// Random bytes from the service's own entropy source. Engines draw new
/// incarnations from it and never generate randomness themselves.
pub type Entropy = Arc<dyn Fn() -> [u8; 32] + Send + Sync>;

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
        let drawn = entropy();
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
    pub(crate) fn draw(&self) -> [u8; 32] {
        (self.entropy)()
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
