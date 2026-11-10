use super::*;
use crate::{
    cursor::{HoldingPeriod, Sequence},
    identity::Digest,
};
use fastctx_storage::rusqlite::{OptionalExtension, params};

pub(super) fn cursor(db: &Connection, key: &SourceUploadKey) -> SqlResult<Option<SourceUploadState>> {
    let row=db.query_row("SELECT holding,through,prefix_digest,stop_reason FROM sync_source_uploads WHERE hub=?1 AND scope=?2 AND stream=?3 AND epoch=?4",
        params![key.scope.hub.to_string(),key.scope.scope.as_str(),key.stream.as_str(),key.epoch.as_str()],|r|Ok((r.get::<_,Vec<u8>>(0)?,sql::counter(r,1)?,r.get::<_,Option<String>>(2)?,r.get::<_,Option<Vec<u8>>>(3)?))).optional()?;
    row.map(|(holding, through, prefix, stopped)| {
        let stopped = stopped.as_deref().map(decode).transpose()?;
        Ok(SourceUploadState {
            key: key.clone(),
            holding: decode(&holding)?,
            through: Sequence(through),
            prefix_digest: prefix.map(Digest::try_from).transpose().map_err(sql::invalid)?,
            proof: None,
            phase: if stopped.is_some() { SourceUploadPhase::Stopped } else { SourceUploadPhase::Idle },
            receipt: None,
            stopped,
        })
    })
    .transpose()
}
pub(super) fn load(db: &Connection, key: &SourceUploadKey) -> SqlResult<Option<SourceUploadState>> {
    let Some(state) = cursor(db, key)? else {
        return Ok(None);
    };
    if state.stopped.is_some() {
        return Ok(Some(state));
    }
    let proof=db.query_row("SELECT proof FROM sync_source_upload_batches WHERE hub=?1 AND scope=?2 AND stream=?3 AND epoch=?4 ORDER BY after_sequence LIMIT 1",
        params![key.scope.hub.to_string(),key.scope.scope.as_str(),key.stream.as_str(),key.epoch.as_str()],|r|r.get::<_,String>(0)).optional()?;
    if let Some(proof) = proof {
        let next = batch(db, &state, &Digest::try_from(proof).map_err(sql::invalid)?)?
            .ok_or_else(|| sql::invalid("upload batch disappeared"))?;
        if next.through != state.through {
            return Err(sql::invalid("pending source batch is not contiguous with its acknowledgement cursor"));
        }
        return Ok(Some(next));
    }
    Ok(Some(state))
}
pub(super) fn batch(
    db: &Connection,
    stream: &SourceUploadState,
    proof: &Digest,
) -> SqlResult<Option<SourceUploadState>> {
    let key = &stream.key;
    let row=db.query_row("SELECT after_sequence,phase,receipt FROM sync_source_upload_batches WHERE hub=?1 AND scope=?2 AND stream=?3 AND epoch=?4 AND proof=?5",
        params![key.scope.hub.to_string(),key.scope.scope.as_str(),key.stream.as_str(),key.epoch.as_str(),proof.as_str()],|r|Ok((sql::counter(r,0)?,r.get::<_,String>(1)?,r.get::<_,Option<Vec<u8>>>(2)?))).optional()?;
    row.map(|(after, phase, receipt)| {
        let mut state = stream.clone();
        state.proof = Some(proof.clone());
        state.phase = SourceUploadPhase::parse(&phase)?;
        state.receipt = receipt.as_deref().map(decode).transpose()?;
        if state.through != Sequence(after) {
            state.prefix_digest = None;
        }
        state.through = Sequence(after);
        Ok(state)
    })
    .transpose()
}
pub(super) fn begin(db: &Connection, key: &SourceUploadKey, holding: &HoldingPeriod) -> SqlResult<SourceUploadState> {
    db.execute(
        "INSERT INTO sync_source_uploads(hub,scope,stream,epoch,holding,through) VALUES(?1,?2,?3,?4,?5,?6)",
        params![
            key.scope.hub.to_string(),
            key.scope.scope.as_str(),
            key.stream.as_str(),
            key.epoch.as_str(),
            encode(holding)?,
            0_u64.to_be_bytes().as_slice()
        ],
    )?;
    load(db, key)?.ok_or_else(|| sql::invalid("source upload cursor disappeared"))
}
pub(super) fn stop(db: &Connection, state: &SourceUploadState, reason: &SourceUploadStop) -> SqlResult<()> {
    db.execute("UPDATE sync_source_uploads SET stop_reason=?5 WHERE hub=?1 AND scope=?2 AND stream=?3 AND epoch=?4 AND stop_reason IS NULL",
        params![state.key.scope.hub.to_string(),state.key.scope.scope.as_str(),state.key.stream.as_str(),state.key.epoch.as_str(),encode(reason)?])?;
    Ok(())
}
pub(super) fn save(db: &Connection, state: &SourceUploadState) -> SqlResult<()> {
    let k = &state.key;
    if let Some(proof) = &state.proof {
        db.execute("INSERT INTO sync_source_upload_batches(hub,scope,stream,epoch,proof,after_sequence,phase,receipt) VALUES(?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT(hub,scope,stream,epoch,proof) DO UPDATE SET phase=excluded.phase,receipt=excluded.receipt",
            params![k.scope.hub.to_string(),k.scope.scope.as_str(),k.stream.as_str(),k.epoch.as_str(),proof.as_str(),state.through.0.to_be_bytes().as_slice(),state.phase.name(),state.receipt.as_ref().map(encode).transpose()?])?;
    } else {
        let changed=db.execute("UPDATE sync_source_uploads SET through=?5,prefix_digest=?6 WHERE hub=?1 AND scope=?2 AND stream=?3 AND epoch=?4 AND through<?5",
            params![k.scope.hub.to_string(),k.scope.scope.as_str(),k.stream.as_str(),k.epoch.as_str(),state.through.0.to_be_bytes().as_slice(),state.prefix_digest.as_ref().map(Digest::as_str)])?;
        if changed != 1 {
            return Err(fastctx_storage::Error::with_kind(
                fastctx_storage::ErrorKind::Busy,
                "source acknowledgement no longer advances the cursor",
            ));
        }
        db.execute("DELETE FROM sync_source_upload_batches WHERE hub=?1 AND scope=?2 AND stream=?3 AND epoch=?4 AND after_sequence<?5",
            params![k.scope.hub.to_string(),k.scope.scope.as_str(),k.stream.as_str(),k.epoch.as_str(),state.through.0.to_be_bytes().as_slice()])?;
    }
    Ok(())
}
