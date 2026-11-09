//! A producer's stream epoch exists before anything is staged under it, so a
//! restarted producer resumes the epoch it began rather than opening another.
use super::{SqliteSources, commit::encode};
use crate::{
    identity::{ObjectKey, Producer, ProducerEpoch, StreamId},
    registry::RegisteredKind,
    store::{
        Error, FutureResult,
        boundary::{self, Role},
        sql,
    },
};
use fastctx_storage::rusqlite::{OptionalExtension, params};

/// The stream a producer is about to sequence, with a fresh epoch drawn from the
/// caller's own entropy. Only the first candidate the engine persists is kept.
pub struct StreamOpening {
    pub object: ObjectKey,
    pub kind: RegisteredKind,
    pub producer: Producer,
    pub candidate: ProducerEpoch,
}

enum Opened {
    Epoch(ProducerEpoch),
    Closed,
}

impl SqliteSources {
    /// The epoch this World sequences the object's stream under. The first call
    /// persists `candidate` with an empty head; every later call returns it. A
    /// stream that already holds several epochs is continued only by a handover.
    pub fn open_stream(&self, opening: StreamOpening) -> FutureResult<'_, ProducerEpoch> {
        Box::pin(async move {
            let registration = opening.kind.registration();
            let stream = StreamId::of(&opening.object, registration.id);
            let producer = encode(&opening.producer).map_err(|error| Error::Invalid(error.to_string()))?;
            let (kind, schema) = (registration.id, registration.schema);
            let StreamOpening { object, candidate, .. } = opening;
            let opened = self
                .blocks
                .database()
                .write(4096, move |db| {
                    let _boundary = boundary::enter_typed_write(Role::Source);
                    let (hub, scope) = (object.scope.hub.to_string(), object.scope.scope.as_str());
                    let enabled: Option<bool> = db
                        .prepare_cached("SELECT enabled FROM sync_scopes WHERE hub=?1 AND scope=?2")?
                        .query_row(params![hub, scope], |row| row.get(0))
                        .optional()?;
                    match enabled {
                        None => return Err(sql::invalid("the stream's scope has not been opened")),
                        Some(false) => return Ok(Opened::Closed),
                        Some(true) => {}
                    }
                    let mut heads = db.prepare_cached(
                        "SELECT object,producer,kind,schema,epoch FROM sync_stream_heads WHERE hub=?1 AND scope=?2 AND stream=?3 LIMIT 2",
                    )?;
                    let found = heads
                        .query_map(params![hub, scope, stream.as_str()], |row| {
                            Ok((
                                row.get::<_, String>(0)?,
                                row.get::<_, Vec<u8>>(1)?,
                                row.get::<_, String>(2)?,
                                row.get::<_, u16>(3)?,
                                row.get::<_, String>(4)?,
                            ))
                        })?
                        .collect::<Result<Vec<_>, _>>()?;
                    match found.as_slice() {
                        [] => {
                            let zero = 0_u64.to_be_bytes();
                            db.prepare_cached("INSERT INTO sync_stream_heads(hub,scope,stream,object,producer,epoch,kind,schema,sequence,local,epoch_proof,prefix_digest) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?9,NULL,NULL)")?
                                .execute(params![
                                    hub,
                                    scope,
                                    stream.as_str(),
                                    object.object.as_str(),
                                    producer,
                                    candidate.as_str(),
                                    kind,
                                    schema,
                                    zero.as_slice()
                                ])?;
                            Ok(Opened::Epoch(candidate))
                        }
                        [(held, by, of, version, epoch)] => {
                            if held != object.object.as_str() || by != &producer || of != kind || *version != schema {
                                return Err(sql::invalid("the stream belongs to another object, producer or kind"));
                            }
                            let epoch = ProducerEpoch::try_from(epoch.clone()).map_err(sql::invalid)?;
                            Ok(Opened::Epoch(epoch))
                        }
                        _ => Err(sql::invalid("a stream continued under several epochs needs its handover")),
                    }
                })
                .await
                .map_err(|error| sql::error(error, self.blocks.worker().clone()))?;
            match opened {
                Opened::Epoch(epoch) => Ok(epoch),
                Opened::Closed => Err(Error::Revoked),
            }
        })
    }
}
