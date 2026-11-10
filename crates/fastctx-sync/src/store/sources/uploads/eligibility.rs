//! Only installed directory scopes and streams produced here may leave this node.
use super::*;
use crate::{
    catalog::Lifecycle,
    cursor::{Holder, HoldingPeriod},
    identity::{AccountId, ObjectId, ObjectKey, Producer},
    registry::REGISTRY,
    scope::{ScopeOwner, ScopeSubject},
    store::scopes,
    transfer::KindVersion,
};
use fastctx_storage::rusqlite::{OptionalExtension, params};
pub(super) struct Route {
    pub holding: HoldingPeriod,
    pub object: ObjectKey,
    pub kind: KindVersion,
    pub account: AccountId,
    pub enabled: bool,
    pub through: crate::cursor::Sequence,
}
impl SourceUploads {
    pub(super) fn route(&self, db: &Connection, key: &SourceUploadKey) -> SqlResult<Option<Route>> {
        let row=db.query_row("SELECT object,producer,kind,schema,origin,sequence FROM sync_stream_heads WHERE hub=?1 AND scope=?2 AND stream=?3 AND epoch=?4",
            params![key.scope.hub.to_string(),key.scope.scope.as_str(),key.stream.as_str(),key.epoch.as_str()],|r|Ok((r.get::<_,String>(0)?,r.get::<_,Vec<u8>>(1)?,r.get::<_,String>(2)?,r.get::<_,u16>(3)?,r.get::<_,String>(4)?,sql::counter(r,5)?))).optional()?;
        let Some((object, producer, kind, schema, origin, through)) = row else { return Ok(None) };
        if origin != "local" || decode::<Producer>(&producer)? != (Producer::Device { device: self.device.clone() }) {
            return Ok(None);
        }
        let Some(current) = scopes::current(db, &key.scope)? else { return Ok(None) };
        let Some(descriptor) = scopes::directory_description(db, &key.scope)? else { return Ok(None) };
        let (ScopeOwner::Account { identity }, ScopeSubject::Conversation { conversation, .. }) =
            (&descriptor.owner, &descriptor.subject)
        else {
            return Ok(None);
        };
        let object = ObjectKey { scope: key.scope.clone(), object: ObjectId::try_from(object).map_err(sql::invalid)? };
        let canonical =
            fastctx_types::sync::conversation_identity::source_object(conversation, &kind).ok_or_else(|| {
                fastctx_storage::Error::with_kind(
                    fastctx_storage::ErrorKind::Unavailable,
                    "source kind has no canonical conversation object contract",
                )
            })?;
        if object.object != canonical
            || crate::identity::StreamId::of(&object, &kind) != key.stream
            || REGISTRY.asset(&kind).map_err(sql::invalid)?.registration().schema != schema
        {
            return Err(sql::invalid("local source differs from its directory object or registered stream"));
        }
        Ok(Some(Route {
            holding: HoldingPeriod {
                holder: Holder::Device { device: self.device.clone() },
                scope: key.scope.clone(),
                acl_revision: current.acl_revision,
                publication_epoch: current.publication_epoch,
            },
            object,
            through: crate::cursor::Sequence(through),
            kind: KindVersion { kind, schema },
            account: identity.id.clone(),
            enabled: current.enabled
                && descriptor.lifecycle == Lifecycle::Live
                && descriptor.acl_revision == current.acl_revision,
        }))
    }
    pub(crate) async fn keys(&self, after: Option<SourceUploadKey>, max: u32) -> Result<Vec<SourceUploadKey>, Error> {
        if max == 0 || max > 64 {
            return Err(Error::Invalid("upload polls hold 1 to 64 streams".into()));
        }
        if let Some(key) = &after {
            self.check_hub(key)?;
        }
        let hub = self.hub.clone();
        let producer = encode(&Producer::Device { device: self.device.clone() })
            .map_err(|e| sql::error(e, self.sources.blocks.worker().clone()))?;
        self.sources.blocks.database().read(move|db|{
            let keys=db.prepare_cached("SELECT scope,stream,epoch FROM sync_stream_heads WHERE hub=?1 AND origin='local' AND producer=?2 AND (scope,stream,epoch)>(?3,?4,?5) ORDER BY scope,stream,epoch LIMIT ?6")?
                .query_map(params![hub.to_string(),producer,after.as_ref().map_or("",|k|k.scope.scope.as_str()),after.as_ref().map_or("",|k|k.stream.as_str()),after.as_ref().map_or("",|k|k.epoch.as_str()),max],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?)))?.collect::<fastctx_storage::rusqlite::Result<Vec<_>>>()?;
            keys.into_iter().map(|(scope,stream,epoch)|Ok(SourceUploadKey{scope:crate::identity::ScopeKey{hub:hub.clone(),scope:crate::identity::ScopeId::try_from(scope).map_err(sql::invalid)?},stream:crate::identity::StreamId::try_from(stream).map_err(sql::invalid)?,epoch:crate::identity::ProducerEpoch::try_from(epoch).map_err(sql::invalid)?})).collect()
        }).await.map_err(|e|sql::error(e,self.sources.blocks.worker().clone()))
    }
}
