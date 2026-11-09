//! Content an authority writer stages under its own operation is checked like
//! any supply before the writer's transaction, then adopted inside it.
use super::{
    SqliteBlocks, source_commit, source_supply,
    supply::{self, SupplyOwner, SupplyRequest, SupplyTicket},
};
use crate::{
    identity::{OperationId, StreamId},
    manifest::ContentRef,
    store::{Error, access::Custody, sql},
};
use fastctx_storage::rusqlite::Connection;
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

/// Finished supply checks, one for each root a writer is about to retain.
pub(crate) struct AuthoritySupplies {
    tickets: BTreeMap<Vec<u8>, SupplyTicket>,
}

/// Checks every root's closure against what `operation` staged for its object.
/// The records `operation` writes into `stream` must retain each root.
pub(crate) async fn prepare(
    blocks: &SqliteBlocks,
    operation: &OperationId,
    stream: &StreamId,
    roots: &[(Custody, ContentRef)],
) -> Result<AuthoritySupplies, Error> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut tickets = BTreeMap::new();
    for (custody, root) in roots {
        let key = key(custody, root).map_err(|error| sql::error(error, operation.clone()))?;
        if tickets.contains_key(&key) {
            continue;
        }
        let request = SupplyRequest {
            operation: operation.clone(),
            custody: custody.clone(),
            root: root.clone(),
            bases: Vec::new(),
            owner: SupplyOwner::Stream { stream: stream.clone() },
        };
        tickets.insert(key, source_supply::check(blocks, request, deadline).await?);
    }
    Ok(AuthoritySupplies { tickets })
}

impl AuthoritySupplies {
    /// Retains `root` for `custody` inside the writer's transaction.
    pub(crate) fn adopt(
        &self,
        db: &Connection,
        blocks: &SqliteBlocks,
        custody: &Custody,
        root: &ContentRef,
    ) -> fastctx_storage::Result<()> {
        let ticket = self
            .tickets
            .get(&key(custody, root)?)
            .ok_or_else(|| sql::invalid("content was not prepared for adoption"))?;
        let writer = supply::writer(db, blocks.holder(), blocks.policy().as_ref(), source_supply::now())?;
        let ready = writer.recheck(custody, ticket)?;
        writer.retain_supplied(ready)?;
        source_commit::expand_descriptor(db, custody, root)
    }
}

fn key(custody: &Custody, root: &ContentRef) -> fastctx_storage::Result<Vec<u8>> {
    serde_json::to_vec(&(&custody.object, custody.kind.registration().id, root))
        .map_err(|error| sql::invalid(error.to_string()))
}
