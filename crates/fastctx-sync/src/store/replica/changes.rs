//! Feed hashes are checked over the unchanged authority heads. Local completeness
//! and custody are decided only after that check, inside the receiving transaction.
use super::{
    SqliteReplica,
    rows::{self, decode, encode},
};
use crate::{
    catalog::{ChangesPage, FeedEntry},
    cursor::{LocalSequence, ScopeCursor},
    identity::Digest,
    store::{
        Error, FutureResult,
        boundary::{self, Role},
        local_head,
        replica::{ReplicaCommit, ReplicaContext},
        sql,
    },
};
use fastctx_storage::rusqlite::{OptionalExtension, params};
use fastctx_types::sync::{feed, transfer::ViewDefinition};
use std::collections::BTreeMap;

impl SqliteReplica {
    /// Initializes only a zero cursor obtained from the authenticated hub. A
    /// nonzero starting position requires the separately fenced catalog path.
    pub fn begin(
        &self,
        context: ReplicaContext,
        cursor: ScopeCursor,
        view: ViewDefinition,
    ) -> FutureResult<'_, ReplicaCommit> {
        Box::pin(async move {
            self.connection(&context)?;
            if cursor.scope != context.holding.scope
                || cursor.acl_revision != context.holding.acl_revision
                || view.scope != cursor.scope
                || view.acl_revision != cursor.acl_revision
                || cursor.view.as_ref() != Some(&view.id().map_err(Error::Invalid)?)
                || cursor.through.0 != 0
                || cursor.prefix_digest.as_ref() != Some(&feed::genesis(&cursor.scope, &cursor.epoch))
            {
                return Err(Error::Invalid("initial replica cursor must be the negotiated view's genesis".into()));
            }
            let bytes = encode(&view).map_err(|e| sql::error(e, self.blocks.worker().clone()))?;
            if bytes.len() > fastctx_types::sync::PAGE_BYTES as usize {
                return Err(Error::Invalid("replica view exceeds one page".into()));
            }
            for kind in &view.kinds {
                let registered = crate::registry::REGISTRY.asset(&kind.kind).map_err(Error::Invalid)?;
                if registered.registration().schema != kind.schema {
                    return Err(Error::Invalid("view contains an unsupported schema".into()));
                }
            }
            self.blocks.database().write(bytes.len()+4096,move |db| {
                let _role=boundary::enter_typed_write(Role::Replica);
                rows::current(db,&context.holding)?;
                if let Some((stored,local))=rows::cursor(db,&context.holding)? {
                    if stored.epoch!=cursor.epoch || stored.view!=cursor.view {return Err(sql::invalid("view/feed reconciliation is required"));}
                    return Ok(ReplicaCommit {local,cursor:Some(stored),next:None});
                }
                let local=local_head::current(db)?;
                db.prepare_cached("INSERT INTO sync_replica_scopes(hub,scope,holding,cursor,view,local) VALUES(?1,?2,?3,?4,?5,?6)")?
                    .execute(params![cursor.scope.hub.to_string(),cursor.scope.scope.as_str(),encode(&context.holding)?,encode(&cursor)?,bytes,local.0.to_be_bytes().as_slice()])?;
                Ok(ReplicaCommit {local,cursor:Some(cursor),next:None})
            }).await.map_err(|e|sql::error(e,self.blocks.worker().clone()))
        })
    }
    pub fn cursor(&self, context: ReplicaContext) -> FutureResult<'_, Option<ScopeCursor>> {
        Box::pin(async move {
            self.connection(&context)?;
            self.blocks
                .database()
                .read(move |db| {
                    rows::current(db, &context.holding)?;
                    rows::cursor(db, &context.holding).map(|x| x.map(|x| x.0))
                })
                .await
                .map_err(|e| sql::error(e, self.blocks.worker().clone()))
        })
    }
    pub(super) async fn changes(&self, context: ReplicaContext, page: ChangesPage) -> Result<ReplicaCommit, Error> {
        self.connection(&context)?;
        let bytes = encode(&page).map_err(|e| sql::error(e, self.blocks.worker().clone()))?;
        if bytes.len() > fastctx_types::sync::PAGE_BYTES as usize || page.entries.len() > 256 {
            return Err(Error::Invalid("changes page exceeds its byte or entry budget".into()));
        }
        let digest = Digest::of(&bytes);
        validate(&context, &page)?;
        let mut proofs = BTreeMap::new();
        for signed in &page.proofs {
            if proofs.insert(signed.proof.id(), signed.clone()).is_some() {
                return Err(Error::Invalid("duplicate acceptance proof in one page".into()));
            }
        }
        let mut accepted = BTreeMap::new();
        for entry in &page.entries {
            if let FeedEntry::Changed { change, .. } = entry
                && let crate::catalog::HeadEvidence::Accepted { proof } = &change.head.evidence
            {
                if !accepted.contains_key(proof) {
                    let evidence = super::heads::verify(self.verifier.as_ref(), change, &proofs)?
                        .ok_or_else(|| Error::Invalid("accepted head has no evidence".into()))?;
                    accepted.insert(proof.clone(), evidence);
                }
                super::heads::match_head(change, &accepted[proof])?;
            }
        }
        if accepted.len() != proofs.len() {
            return Err(Error::Invalid("changes page contains unreferenced acceptance evidence".into()));
        }
        let blocks = self.blocks.clone();
        self.blocks.database().write(bytes.len()*4+16384,move |db| {
            let _role=boundary::enter_typed_write(Role::Replica);
            rows::current(db,&context.holding)?;
            let period=encode(&context.holding)?;
            let previous=db.prepare_cached("SELECT local,cursor FROM sync_replica_pages WHERE hub=?1 AND scope=?2 AND holding=?3 AND page=?4")?
                .query_row(params![context.holding.scope.hub.to_string(),context.holding.scope.scope.as_str(),period,digest.as_str()],|r|Ok((sql::counter(r,0)?,r.get::<_,Vec<u8>>(1)?))).optional()?;
            if let Some((local,cursor))=previous {return Ok(ReplicaCommit{local:LocalSequence(local),cursor:Some(decode(&cursor)?),next:None});}
            let (stored,_)=rows::cursor(db,&context.holding)?.ok_or_else(||sql::invalid("initialize the replica view before receiving changes"))?;
            if stored!=page.after {return Err(sql::invalid("changes page does not continue the durable replica cursor"));}
            let view:Vec<u8>=db.query_row("SELECT view FROM sync_replica_scopes WHERE hub=?1 AND scope=?2",params![stored.scope.hub.to_string(),stored.scope.scope.as_str()],|r|r.get(0))?;
            let view:ViewDefinition=decode(&view)?;
            for entry in &page.entries {
                let FeedEntry::Changed{change,..}=entry else {continue};
                if !view.kinds.iter().any(|k|k.kind==change.head.kind && k.schema==change.head.schema) {
                    return Err(sql::invalid("changed kind/schema is outside the negotiated view"));
                }
                super::heads::install(db,&blocks,&context,change,match &change.head.evidence {crate::catalog::HeadEvidence::Accepted{proof}=>accepted.get(proof),crate::catalog::HeadEvidence::Stream{..}=>None})?;
            }
            let local=local_head::advance(db)?;
            db.execute("UPDATE sync_replica_scopes SET cursor=?3,local=?4 WHERE hub=?1 AND scope=?2",params![stored.scope.hub.to_string(),stored.scope.scope.as_str(),encode(&page.through)?,local.0.to_be_bytes().as_slice()])?;
            db.execute("INSERT INTO sync_replica_pages(hub,scope,holding,page,local,cursor) VALUES(?1,?2,?3,?4,?5,?6)",params![stored.scope.hub.to_string(),stored.scope.scope.as_str(),period,digest.as_str(),local.0.to_be_bytes().as_slice(),encode(&page.through)?])?;
            Ok(ReplicaCommit{local,cursor:Some(page.through),next:None})
        }).await.map_err(|e|sql::error(e,self.blocks.worker().clone()))
    }
}
fn validate(context: &ReplicaContext, page: &ChangesPage) -> Result<(), Error> {
    let mut cursor = page.after.clone();
    if cursor.scope != context.holding.scope
        || cursor.acl_revision != context.holding.acl_revision
        || cursor.view.is_none()
        || cursor.prefix_digest.is_none()
    {
        return Err(Error::Invalid("changes require a current, reconciled cursor".into()));
    }
    for entry in &page.entries {
        match entry {
            FeedEntry::Changed { change, prefix_digest } => {
                if cursor.through.0.checked_add(1) != Some(change.sequence.0)
                    || change.head.key.scope != cursor.scope
                    || change.head.acl_revision > cursor.acl_revision
                {
                    return Err(Error::Invalid("changes are not consecutive in this scope and ACL".into()));
                }
                let (_, next) =
                    feed::advance(cursor.prefix_digest.as_ref().unwrap(), change).map_err(Error::Invalid)?;
                if &next != prefix_digest {
                    return Err(Error::Corrupt("authority change prefix differs from its canonical record".into()));
                }
                cursor.through = change.sequence;
                cursor.prefix_digest = Some(next);
            }
            FeedEntry::Skipped { after, through, view, prefix_digest } => {
                if *after != cursor.through || *through <= *after || cursor.view.as_ref() != Some(view) {
                    return Err(Error::Invalid("filtered feed advance has a gap or another view".into()));
                }
                // Hidden records cannot be rehashed by this view. The authenticated
                // hub supplies their original anchor, never a hash of a projection.
                cursor.through = *through;
                cursor.prefix_digest = Some(prefix_digest.clone());
            }
        }
    }
    if cursor != page.through {
        return Err(Error::Invalid("changes page boundary differs from its entries".into()));
    }
    Ok(())
}
