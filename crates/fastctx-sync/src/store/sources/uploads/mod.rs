//! Device upload cursors and bounded closure work preserve original source proofs.
mod eligibility;
mod nodes;
mod proofs;
mod receipts;
mod rows;
mod types;
use super::SqliteSources;
use crate::{
    identity::{DeviceId, HubId},
    proof::{HubSigningIdentity, ProofVerifier},
    store::{
        Error,
        boundary::{self, Role},
        sql,
    },
};
use fastctx_storage::{Result as SqlResult, rusqlite::Connection};
pub(crate) use nodes::UploadNode;
pub(crate) use proofs::UploadBatch;
use std::sync::Arc;
pub use types::{SourceUploadKey, SourceUploadPhase, SourceUploadState, SourceUploadStop};

#[derive(Clone)]
pub(crate) struct SourceUploads {
    sources: SqliteSources,
    hub: HubId,
    device: DeviceId,
    identity: HubSigningIdentity,
    verifier: Arc<dyn ProofVerifier>,
}
impl SourceUploads {
    pub(crate) fn new(sources: SqliteSources, hub: HubId) -> Result<Self, Error> {
        let crate::cursor::Holder::Device { device } = sources.blocks.holder() else {
            return Err(Error::Invalid("only a device uploads original source batches".into()));
        };
        let device = device.clone();
        let (identity, verifier) = sources
            .hubs
            .get(&hub)
            .ok_or_else(|| Error::Unavailable("source uploads require a pinned hub verifier".into()))?
            .clone();
        Ok(Self { sources, hub, device, identity, verifier })
    }
    pub(crate) fn budget(&self) -> crate::admission::ByteBudget {
        self.sources.blocks.buffer_budget()
    }
    pub(crate) async fn at(
        &self,
        stream: &SourceUploadState,
        after: crate::cursor::Sequence,
    ) -> Result<SourceUploadState, Error> {
        let mut state = stream.clone();
        state.through = after;
        state.proof = None;
        state.phase = SourceUploadPhase::Idle;
        state.receipt = None;
        if after != stream.through {
            state.prefix_digest = None;
        }
        self.sources.blocks.database().read(move|db| snapshot(db,|db| {
            use fastctx_storage::rusqlite::{params,OptionalExtension};
            let k=&state.key;
            let proof=db.query_row("SELECT proof FROM sync_source_upload_batches WHERE hub=?1 AND scope=?2 AND stream=?3 AND epoch=?4 AND after_sequence=?5",
                params![k.scope.hub.to_string(),k.scope.scope.as_str(),k.stream.as_str(),k.epoch.as_str(),after.0.to_be_bytes().as_slice()],|r|r.get::<_,String>(0)).optional()?;
            if let Some(proof)=proof {rows::batch(db,&state,&crate::identity::Digest::try_from(proof).map_err(sql::invalid)?)?.ok_or_else(||sql::invalid("upload batch disappeared"))} else {Ok(state)}
        })).await.map_err(|e|sql::error(e,self.sources.blocks.worker().clone()))
    }
    fn check_hub(&self, key: &SourceUploadKey) -> Result<(), Error> {
        if key.scope.hub != self.hub {
            return Err(Error::Denied("upload belongs to another hub".into()));
        }
        Ok(())
    }
    pub(crate) async fn status(&self, key: SourceUploadKey) -> Result<Option<SourceUploadState>, Error> {
        self.check_hub(&key)?;
        self.sources
            .blocks
            .database()
            .read(move |db| snapshot(db, |db| rows::load(db, &key)))
            .await
            .map_err(|e| sql::error(e, self.sources.blocks.worker().clone()))
    }
    pub(crate) async fn load(&self, key: SourceUploadKey) -> Result<Option<SourceUploadState>, Error> {
        self.check_hub(&key)?;
        let store = self.clone();
        let read = key.clone();
        let (state, route) = self
            .sources
            .blocks
            .database()
            .read(move |db| {
                let tx = db.unchecked_transaction()?;
                let value = (rows::load(&tx, &read)?, store.route(&tx, &read)?);
                tx.commit()?;
                Ok(value)
            })
            .await
            .map_err(|e| sql::error(e, self.sources.blocks.worker().clone()))?;
        if let Some(state) = &state {
            if state.stopped.is_some() {
                return Ok(Some(state.clone()));
            }
            if let Some(route) = &route
                && route.enabled
                && route.holding == state.holding
            {
                return Ok(Some(state.clone()));
            }
        } else if route.is_none() {
            return Ok(None);
        }
        let store = self.clone();
        self.sources
            .blocks
            .database()
            .write(16384, move |db| {
                let _role = boundary::enter_typed_write(Role::SourceUpload);
                let current = rows::load(db, &key)?;
                let route = store.route(db, &key)?;
                let Some(route) = route else {
                    if let Some(current) = current {
                        rows::stop(db, &current, &SourceUploadStop::NotPromoted)?;
                        return rows::load(db, &key);
                    }
                    return Ok(None);
                };
                let state = match current {
                    Some(state) => state,
                    None => rows::begin(db, &key, &route.holding)?,
                };
                if !route.enabled {
                    rows::stop(db, &state, &SourceUploadStop::Revoked)?;
                } else if route.holding != state.holding {
                    rows::stop(db, &state, &SourceUploadStop::HoldingChanged)?;
                }
                rows::load(db, &key)
            })
            .await
            .map_err(|e| sql::error(e, self.sources.blocks.worker().clone()))
    }
    pub(crate) async fn check(&self, state: &SourceUploadState) -> Result<(), Error> {
        let store = self.clone();
        let expected = state.clone();
        let reason = self
            .sources
            .blocks
            .database()
            .read(move |db| snapshot(db, |db| store.check_in(db, &expected)))
            .await
            .map_err(|e| sql::error(e, self.sources.blocks.worker().clone()))?;
        if let Some(reason) = reason {
            self.stop(state, reason).await?;
            return Err(Error::Revoked);
        }
        Ok(())
    }
    fn check_in(&self, db: &Connection, state: &SourceUploadState) -> SqlResult<Option<SourceUploadStop>> {
        let current = rows::cursor(db, &state.key)?.ok_or_else(|| sql::invalid("upload cursor disappeared"))?;
        if current.holding != state.holding {
            return Ok(Some(SourceUploadStop::HoldingChanged));
        }
        if current.stopped.is_some() {
            return Ok(current.stopped);
        }
        if state.through < current.through {
            return Err(fastctx_storage::Error::with_kind(
                fastctx_storage::ErrorKind::Busy,
                "source batch was already acknowledged",
            ));
        }
        if state.proof.is_some()
            && rows::batch(db, &current, state.proof.as_ref().expect("checked proof"))?
                .is_none_or(|batch| batch.phase != state.phase || batch.receipt != state.receipt)
        {
            return Err(fastctx_storage::Error::with_kind(
                fastctx_storage::ErrorKind::Busy,
                "another uploader advanced this stream",
            ));
        }
        let current = crate::store::scopes::current(db, &state.key.scope)?;
        if current.is_none_or(|s| !s.enabled) {
            return Ok(Some(SourceUploadStop::Revoked));
        }
        let Some(route) = self.route(db, &state.key)? else {
            return Ok(Some(SourceUploadStop::NotPromoted));
        };
        if !route.enabled {
            return Ok(Some(SourceUploadStop::Revoked));
        }
        if route.holding != state.holding {
            return Ok(Some(SourceUploadStop::HoldingChanged));
        }
        Ok(state.stopped.clone())
    }
    async fn change<T: Send + 'static>(
        &self,
        state: &SourceUploadState,
        bytes: usize,
        change: impl FnOnce(&Connection) -> SqlResult<T> + Send + 'static,
    ) -> Result<T, Error> {
        let expected = state.clone();
        let store = self.clone();
        let result = self
            .sources
            .blocks
            .database()
            .write(bytes, move |db| {
                let _role = boundary::enter_typed_write(Role::SourceUpload);
                if let Some(reason) = store.check_in(db, &expected)? {
                    rows::stop(db, &expected, &reason)?;
                    return Ok(None);
                }
                change(db).map(Some)
            })
            .await
            .map_err(|e| sql::error(e, self.sources.blocks.worker().clone()))?;
        result.ok_or(Error::Revoked)
    }
    pub(crate) async fn stop(&self, state: &SourceUploadState, mut reason: SourceUploadStop) -> Result<(), Error> {
        match &mut reason {
            SourceUploadStop::Denied { message }
            | SourceUploadStop::Rejected { message }
            | SourceUploadStop::Corrupt { message } => *message = message.chars().take(512).collect(),
            _ => {}
        }
        let expected = state.clone();
        self.sources
            .blocks
            .database()
            .write(8192, move |db| {
                let _role = boundary::enter_typed_write(Role::SourceUpload);
                if rows::load(db, &expected.key)?
                    .is_some_and(|current| current.holding == expected.holding && current.stopped.is_none())
                {
                    rows::stop(db, &expected, &reason)?;
                }
                Ok(())
            })
            .await
            .map_err(|e| sql::error(e, self.sources.blocks.worker().clone()))
    }
}
fn snapshot<T>(db: &Connection, read: impl FnOnce(&Connection) -> SqlResult<T>) -> SqlResult<T> {
    let tx = db.unchecked_transaction()?;
    let value = read(&tx)?;
    tx.commit()?;
    Ok(value)
}
fn encode(value: &impl serde::Serialize) -> SqlResult<Vec<u8>> {
    serde_json::to_vec(value).map_err(|e| sql::invalid(e.to_string()))
}
fn decode<T: serde::de::DeserializeOwned>(value: &[u8]) -> SqlResult<T> {
    serde_json::from_slice(value).map_err(|e| sql::invalid(e.to_string()))
}
