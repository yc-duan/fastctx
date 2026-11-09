//! What an adapter reads to resume a source: its committed checkpoint, its stream
//! progress and its registered index rows, each behind the holder's current custody.
use super::SqliteSources;
use crate::{
    blocks::source_commit,
    cursor::{ByteCount, Revision, Sequence, StreamCursor},
    identity::HostGeneration,
    store::{
        Error,
        ingest::{SourceCheckpoint, SourceTarget, StreamProgress},
        ingest_effects::{SourceFamily, SourceIndexPage, SourceIndexRead, SourceIndexRow},
        sql,
    },
};
use fastctx_storage::{
    Result as SqlResult,
    rusqlite::{Connection, OptionalExtension, params},
};

impl SqliteSources {
    async fn read<T: Send + 'static>(
        &self,
        target: SourceTarget,
        read: impl FnOnce(&Connection, &SourceTarget) -> SqlResult<T> + Send + 'static,
    ) -> Result<T, Error> {
        self.authorizer(&target.custody)?;
        let holder = self.blocks.holder().clone();
        let policy = self.blocks.policy().clone();
        self.blocks
            .database()
            .read(move |db| {
                source_commit::authorize(db, &holder, policy.as_ref(), &target.custody)?;
                read(db, &target)
            })
            .await
            .map_err(|error| sql::error(error, self.blocks.worker().clone()))
    }
    pub(super) async fn read_checkpoint(&self, target: SourceTarget) -> Result<Option<SourceCheckpoint>, Error> {
        self.read(target, checkpoint).await
    }
    pub(super) async fn read_progress(&self, target: SourceTarget) -> Result<Option<StreamProgress>, Error> {
        self.read(target, progress).await
    }
    pub(super) async fn index_row(
        &self,
        target: SourceTarget,
        family: SourceFamily,
        key: Vec<u8>,
    ) -> Result<Option<SourceIndexRow>, Error> {
        if !family.owner_kinds().contains(&target.custody.kind.registration().id) || key.is_empty() || key.len() > 1024
        {
            return Err(Error::Invalid("source index lookup exceeds its registered namespace".into()));
        }
        self.read(target, move |db, target| {
            let scope = &target.custody.object.scope;
            db.prepare_cached("SELECT revision,value FROM sync_source_index WHERE hub=?1 AND scope=?2 AND source=?3 AND family=?4 AND key=?5")?
                .query_row(
                    params![scope.hub.to_string(), scope.scope.as_str(), target.source.as_str(), family.id(), key],
                    |row| Ok(SourceIndexRow { key: key.clone(), revision: Revision(sql::counter(row, 0)?), value: row.get(1)? }),
                )
                .optional()
                .map_err(Into::into)
        })
        .await
    }
    pub(super) async fn index_page(
        &self,
        target: SourceTarget,
        request: SourceIndexRead,
    ) -> Result<SourceIndexPage, Error> {
        request.validate(&target)?;
        self.read(target, move |db, target| page(db, target, &request)).await
    }
}

fn checkpoint(db: &Connection, target: &SourceTarget) -> SqlResult<Option<SourceCheckpoint>> {
    let scope = &target.custody.object.scope;
    let row = db
        .prepare_cached("SELECT generation,revision,committed_offset,parser_version,state FROM sync_sources WHERE hub=?1 AND scope=?2 AND source=?3")?
        .query_row(params![scope.hub.to_string(), scope.scope.as_str(), target.source.as_str()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                sql::counter(row, 1)?,
                sql::counter(row, 2)?,
                row.get::<_, u16>(3)?,
                row.get::<_, Option<Vec<u8>>>(4)?,
            ))
        })
        .optional()?;
    row.map(|(generation, revision, offset, parser_version, state)| {
        Ok(SourceCheckpoint {
            source: target.source.clone(),
            generation: HostGeneration::try_from(generation).map_err(sql::invalid)?,
            revision: Revision(revision),
            committed_offset: ByteCount(offset),
            parser_version,
            state: state
                .map(|bytes| serde_json::from_slice(&bytes).map_err(|error| sql::invalid(error.to_string())))
                .transpose()?,
        })
    })
    .transpose()
}

/// A first local epoch reports its cursor alone; confirmation adds its proof
/// and prefix digest once both exist.
fn progress(db: &Connection, target: &SourceTarget) -> SqlResult<Option<StreamProgress>> {
    let scope = &target.custody.object.scope;
    let row = db
        .prepare_cached("SELECT sequence,epoch_proof,prefix_digest FROM sync_stream_heads WHERE hub=?1 AND scope=?2 AND stream=?3 AND epoch=?4")?
        .query_row(
            params![scope.hub.to_string(), scope.scope.as_str(), target.stream.as_str(), target.epoch.as_str()],
            |row| Ok((sql::counter(row, 0)?, row.get::<_, Option<String>>(1)?, row.get::<_, Option<String>>(2)?)),
        )
        .optional()?;
    row.map(|(sequence, proof, prefix)| {
        let cursor = StreamCursor {
            scope: scope.clone(),
            stream: target.stream.clone(),
            epoch: target.epoch.clone(),
            committed: Sequence(sequence),
        };
        Ok(match (proof, prefix) {
            (Some(proof), Some(prefix)) => StreamProgress::Confirmed {
                cursor,
                epoch_proof: proof.try_into().map_err(sql::invalid)?,
                prefix_digest: prefix.try_into().map_err(sql::invalid)?,
            },
            _ => StreamProgress::LocalPending { cursor },
        })
    })
    .transpose()
}

fn page(db: &Connection, target: &SourceTarget, request: &SourceIndexRead) -> SqlResult<SourceIndexPage> {
    let scope = &target.custody.object.scope;
    let after = request.after.clone().unwrap_or_default();
    let rows = db
        .prepare_cached("SELECT key,revision,value FROM sync_source_index WHERE hub=?1 AND scope=?2 AND source=?3 AND family=?4 AND key>?5 ORDER BY key LIMIT ?6")?
        .query_map(
            params![
                scope.hub.to_string(),
                scope.scope.as_str(),
                target.source.as_str(),
                request.family.id(),
                after,
                request.limit.items + 1
            ],
            |row| Ok(SourceIndexRow { key: row.get(0)?, revision: Revision(sql::counter(row, 1)?), value: row.get(2)? }),
        )?
        .collect::<fastctx_storage::rusqlite::Result<Vec<_>>>()?;
    let mut page = SourceIndexPage { rows: Vec::new(), next: None };
    let mut bytes = 0_usize;
    for row in rows {
        let size = row.key.len() + row.value.len() + 8;
        if page.rows.len() == request.limit.items as usize || bytes + size > request.limit.bytes as usize {
            page.next = page.rows.last().map(|last: &SourceIndexRow| last.key.clone());
            if page.rows.is_empty() {
                return Err(sql::invalid("one source index row exceeds the requested page bytes"));
            }
            break;
        }
        bytes += size;
        page.rows.push(row);
    }
    Ok(page)
}
