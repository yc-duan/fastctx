//! An account's directory stream. The hub orders it, confirms its own epoch and
//! signs one record per batch, so a holder can receive exactly one description.
use crate::{
    catalog::{SourceBatch, SourceRecord},
    cursor::Sequence,
    identity::{Digest, OperationId, Producer, ProducerEpoch, ScopeKey, StreamId},
    manifest::ContentBlock,
    proof::{
        Bytes, ConfirmedEpoch, EpochDescriptor, HandoverBasis, ProofPurpose, ProofSigner, Signature, SignedSourceBatch,
        SignedStatement, SigningKey,
    },
    scope::directory_object,
    store::{control, control::AuthorityRun, sql},
};
use fastctx_storage::{
    Result,
    rusqlite::{Connection, OptionalExtension, params},
};
use fastctx_types::sync::feed;

pub(super) const KIND: &str = "account.directory";
const SCHEMA: u16 = 1;

pub(super) struct Stream {
    pub scope: ScopeKey,
    pub stream: StreamId,
    pub epoch: ProducerEpoch,
    pub sequence: Sequence,
    pub prefix: Digest,
    pub confirmed: ConfirmedEpoch,
}

pub(super) fn id(directory: &ScopeKey) -> StreamId {
    StreamId::of(&directory_object(directory), KIND)
}

/// Opens `directory`'s stream under a new epoch the hub confirms itself.
pub(super) fn open(db: &Connection, run: &AuthorityRun, signer: &dyn ProofSigner, directory: &ScopeKey) -> Result<()> {
    let stream = id(directory);
    let epoch = ProducerEpoch::from_bytes(run.draw()?);
    let seed = format!("fastctx-sync/directory-epoch/v1\0{}\0{}", stream.as_str(), epoch.as_str());
    let registration = OperationId::from_bytes(Digest::of(seed.as_bytes()).bytes());
    let descriptor = EpochDescriptor {
        scope: directory.clone(),
        stream: stream.clone(),
        kind: KIND.into(),
        schema: SCHEMA,
        epoch: epoch.clone(),
        producer: Producer::Hub,
        predecessor: None,
        closed_through: None,
        operation: registration.clone(),
        control_sequence: control::next_control(db, run.hub())?,
        basis: HandoverBasis::Initial { registration: registration.clone() },
    };
    let confirmed = signer.epoch(&descriptor).map_err(|error| sql::invalid(error.to_string()))?;
    let proof = confirmed.proof.id();
    let producer = serde_json::to_vec(&Producer::Hub).map_err(|error| sql::invalid(error.to_string()))?;
    let (hub, scope) = (directory.hub.to_string(), directory.scope.as_str());
    db.prepare_cached("INSERT INTO sync_epoch_descriptors(hub,scope,stream,epoch,producer,kind,schema,predecessor,closed_through,proof,signed_bytes,signer,signature,operation) VALUES(?1,?2,?3,?4,?5,?6,?7,NULL,NULL,?8,?9,?10,?11,?12)")?
        .execute(params![
            hub,
            scope,
            stream.as_str(),
            epoch.as_str(),
            producer,
            KIND,
            SCHEMA,
            proof.as_str(),
            confirmed.proof.data.0,
            confirmed.proof.signer.0.as_slice(),
            confirmed.proof.signature.0.as_slice(),
            registration.as_str()
        ])?;
    let zero = 0_u64.to_be_bytes();
    db.prepare_cached("INSERT INTO sync_stream_heads(hub,scope,stream,object,producer,epoch,kind,schema,sequence,local,epoch_proof,prefix_digest) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?9,?10,?11)")?
        .execute(params![
            hub,
            scope,
            stream.as_str(),
            directory_object(directory).object.as_str(),
            producer,
            epoch.as_str(),
            KIND,
            SCHEMA,
            zero.as_slice(),
            proof.as_str(),
            feed::stream_genesis(&proof).as_str()
        ])?;
    Ok(())
}

/// The stream's head with the epoch confirmation every record proves against.
pub(super) fn head(db: &Connection, directory: &ScopeKey) -> Result<Option<Stream>> {
    let stream = id(directory);
    let (hub, scope) = (directory.hub.to_string(), directory.scope.as_str());
    let found = db
        .prepare_cached("SELECT h.epoch,h.sequence,h.prefix_digest,e.signed_bytes,e.signer,e.signature FROM sync_stream_heads h JOIN sync_epoch_descriptors e USING(hub,scope,stream,epoch) WHERE h.hub=?1 AND h.scope=?2 AND h.stream=?3")?
        .query_row(params![hub, scope, stream.as_str()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                sql::counter(row, 1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Vec<u8>>(3)?,
                row.get::<_, Vec<u8>>(4)?,
                row.get::<_, Vec<u8>>(5)?,
            ))
        })
        .optional()?;
    let Some((epoch, sequence, prefix, data, signer, signature)) = found else { return Ok(None) };
    let prefix = prefix.ok_or_else(|| sql::invalid("a hub directory stream has no prefix"))?;
    Ok(Some(Stream {
        scope: directory.clone(),
        stream,
        epoch: ProducerEpoch::try_from(epoch).map_err(sql::invalid)?,
        sequence: Sequence(sequence),
        prefix: Digest::try_from(prefix).map_err(sql::invalid)?,
        confirmed: ConfirmedEpoch { proof: statement(ProofPurpose::EpochHandoverV1, data, &signer, &signature)? },
    }))
}

/// The batch an earlier attempt of `operation` already signed into the stream.
pub(super) fn recorded(db: &Connection, stream: &Stream, operation: &OperationId) -> Result<Option<SignedSourceBatch>> {
    db.prepare_cached("SELECT purpose,signed_bytes,signer,signature FROM sync_source_proofs WHERE hub=?1 AND scope=?2 AND stream=?3 AND epoch=?4 AND operation=?5")?
        .query_row(
            params![
                stream.scope.hub.to_string(),
                stream.scope.scope.as_str(),
                stream.stream.as_str(),
                stream.epoch.as_str(),
                operation.as_str()
            ],
            |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?, row.get::<_, Vec<u8>>(2)?, row.get::<_, Vec<u8>>(3)?))
            },
        )
        .optional()?
        .map(|(purpose, data, signer, signature)| {
            let purpose = ProofPurpose::from_domain(&purpose).ok_or_else(|| sql::invalid("unknown proof purpose"))?;
            Ok(SignedSourceBatch { proof: statement(purpose, data, &signer, &signature)? })
        })
        .transpose()
}

/// Signs and appends one record carrying `payload`, the stream's next sequence.
/// Returns the batch and the stream's new head.
pub(super) fn append(
    db: &Connection,
    signer: &dyn ProofSigner,
    stream: &Stream,
    operation: &OperationId,
    payload: &ContentBlock,
) -> Result<(SignedSourceBatch, Sequence, Digest)> {
    let through = Sequence(stream.sequence.0.checked_add(1).ok_or_else(|| sql::invalid("the stream is exhausted"))?);
    let record = SourceRecord {
        sequence: through,
        operation: operation.clone(),
        kind: KIND.into(),
        schema: SCHEMA,
        payload: payload.clone(),
        roots: Vec::new(),
        causes: Vec::new(),
    };
    let batch = SourceBatch {
        format: 1,
        scope: stream.scope.clone(),
        stream: stream.stream.clone(),
        producer: Producer::Hub,
        epoch: stream.epoch.clone(),
        operation: operation.clone(),
        kind: KIND.into(),
        schema: SCHEMA,
        after: stream.sequence,
        through,
        records: vec![record.clone()],
    };
    crate::proof::encode_source_batch(&batch).map_err(|error| sql::invalid(error.to_string()))?;
    let signed = signer.source(&batch).map_err(|error| sql::invalid(error.to_string()))?;
    let (_, prefix) = feed::stream_advance(&stream.prefix, &record).map_err(sql::invalid)?;
    let proof = signed.proof.id();
    let producer = serde_json::to_vec(&Producer::Hub).map_err(|error| sql::invalid(error.to_string()))?;
    let (hub, scope, name, epoch) =
        (stream.scope.hub.to_string(), stream.scope.scope.as_str(), stream.stream.as_str(), stream.epoch.as_str());
    let sequence = through.0.to_be_bytes();
    db.prepare_cached("INSERT INTO sync_source_proofs(hub,scope,proof,stream,epoch,producer,operation,first_sequence,last_sequence,purpose,signer,signed_bytes,signature,state) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?8,?9,?10,?11,?12,'accepted')")?
        .execute(params![
            hub,
            scope,
            proof.as_str(),
            name,
            epoch,
            producer,
            operation.as_str(),
            sequence.as_slice(),
            signed.proof.purpose.domain(),
            signed.proof.signer.0.as_slice(),
            signed.proof.data.0,
            signed.proof.signature.0.as_slice()
        ])?;
    db.prepare_cached("INSERT INTO sync_stream_records(hub,scope,stream,epoch,sequence,operation,kind,schema,payload,causes,batch_proof,prefix_digest) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)")?
        .execute(params![
            hub,
            scope,
            name,
            epoch,
            sequence.as_slice(),
            operation.as_str(),
            KIND,
            SCHEMA,
            json(&record.payload)?,
            json(&record.causes)?,
            proof.as_str(),
            prefix.as_str()
        ])?;
    db.prepare_cached("INSERT INTO sync_stream_roots(hub,scope,stream,epoch,sequence,hash,root_kind,decoded_bytes) VALUES(?1,?2,?3,?4,?5,?6,'block',?7)")?
        .execute(params![hub, scope, name, epoch, sequence.as_slice(), payload.id.digest.as_str(), payload.decoded_bytes])?;
    db.prepare_cached("UPDATE sync_stream_heads SET sequence=?5,prefix_digest=?6 WHERE hub=?1 AND scope=?2 AND stream=?3 AND epoch=?4")?
        .execute(params![hub, scope, name, epoch, sequence.as_slice(), prefix.as_str()])?;
    Ok((signed, through, prefix))
}

fn statement(purpose: ProofPurpose, data: Vec<u8>, signer: &[u8], signature: &[u8]) -> Result<SignedStatement> {
    Ok(SignedStatement {
        purpose,
        signer: SigningKey(signer.try_into().map_err(|_| sql::invalid("a stored signer key has a wrong length"))?),
        data: Bytes(data),
        signature: Signature(signature.try_into().map_err(|_| sql::invalid("a stored signature has a wrong length"))?),
    })
}
fn json(value: &impl serde::Serialize) -> Result<Vec<u8>> {
    serde_json::to_vec(value).map_err(|error| sql::invalid(error.to_string()))
}
