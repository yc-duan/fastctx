//! Obsolete waiting intervals are evidence, not a head-of-line blocker for the
//! next legal interval. Retiring one never removes an accepted source record.
use super::{SqliteAuthority, receipts, streams, waiting};
use crate::{
    catalog::SourceBatch,
    identity::{Digest, ObjectKey},
    proof::SignedSourceBatch,
    scope::ScopeSubject,
    store::{
        Error,
        boundary::{self, Role},
        directory, sql,
    },
};
use fastctx_storage::rusqlite::{OptionalExtension, params};

pub(super) async fn overlap(authority: &SqliteAuthority, signed: SignedSourceBatch) -> Result<bool, Error> {
    let batch: SourceBatch = serde_json::from_slice(&signed.proof.data.0).map_err(|e| Error::Invalid(e.to_string()))?;
    let op = batch.operation.clone();
    let run = authority.run.clone();
    authority.blocks.database().write(fastctx_types::sync::PAGE_BYTES as usize*2,move|db|{
        let _boundary=boundary::enter_typed_write(Role::Authority);
        let active:Vec<u8>=db.prepare_cached("SELECT authority_epoch FROM sync_hub_control_heads WHERE hub=?1")?.query_row(params![run.hub().to_string()],|r|r.get(0))?;
        if active!=run.epoch().bytes(){return Err(sql::invalid("this authority run was superseded"));}
        let Some(head)=streams::head(db,&batch)? else{return Ok(false)};
        if batch.after>=head.sequence{return Ok(false);}
        let waiting:bool=db.prepare_cached("SELECT EXISTS(SELECT 1 FROM sync_source_proofs p JOIN sync_authority_waiting w USING(hub,scope,stream,epoch,operation) WHERE p.hub=?1 AND p.scope=?2 AND p.proof=?3 AND p.state='awaiting_epoch')")?
            .query_row(params![batch.scope.hub.to_string(),batch.scope.scope.as_str(),signed.proof.id().as_str()],|r|r.get(0))?;
        if !waiting {return Ok(false);}
        let Some(descriptor)=directory::described(db,&batch.scope)? else{return Ok(false)};
        let ScopeSubject::Conversation{conversation,..}=descriptor.subject else{return Ok(false)};
        let original:String=db.prepare_cached("SELECT batch_proof FROM sync_stream_records WHERE hub=?1 AND scope=?2 AND stream=?3 AND epoch=?4 AND sequence=?5")?
            .query_row(params![batch.scope.hub.to_string(),batch.scope.scope.as_str(),batch.stream.as_str(),batch.epoch.as_str(),(batch.after.0+1).to_be_bytes().as_slice()],|r|r.get(0)).optional()?.ok_or_else(||sql::invalid("accepted stream has a record gap"))?;
        let original=Digest::try_from(original).map_err(sql::invalid)?;
        receipts::record_conflict(db,&ObjectKey{scope:batch.scope.clone(),object:conversation},&batch,&original,&signed)?;
        waiting::remove(db,&batch)?;
        db.prepare_cached("DELETE FROM sync_source_proofs WHERE hub=?1 AND scope=?2 AND proof=?3 AND state='awaiting_epoch'")?
            .execute(params![batch.scope.hub.to_string(),batch.scope.scope.as_str(),signed.proof.id().as_str()])?;
        Ok(true)
    }).await.map_err(|e|sql::error(e,op))
}
