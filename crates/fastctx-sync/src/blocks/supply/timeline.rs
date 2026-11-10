//! A timeline fence retains available pages without claiming complete bodies.
//! Snapshots are derived local state with no read grant of their own: the root
//! must be the object's committed head or already held by a live fence.
use super::*;
use crate::{
    cursor::Completeness,
    identity::OperationId,
    manifest::{ContentRef, ManifestPage},
    store::roots::RootAcquired,
};
use fastctx_storage::rusqlite::OptionalExtension;
use fastctx_types::sync::metadata::MetadataCodec;

impl SupplyWriter<'_> {
    /// None means only that the requested head/fence binding is no longer present.
    pub(crate) fn acquire_timeline(
        &self,
        operation: &OperationId,
        request: &AcquireRoot,
    ) -> Result<Option<RootAcquired>> {
        let _boundary = boundary::enter_typed_write(Role::Supply);
        request.validate().map_err(|e| sql::invalid(e.to_string()))?;
        pins::authorize(self.db, self.holder, self.policy, &request.custody)?;
        let RootUse::Timeline { projection } = request.usage else {
            return Err(sql::invalid("timeline acquisition needs its declared usage"));
        };
        let ContentRef::Manifest { root } = &request.root else {
            return Err(sql::invalid("timeline acquisition needs a manifest"));
        };
        if request.expires_at_millis.is_some_and(|at| at <= self.now) {
            return Err(sql::invalid("timeline fence has already expired"));
        }
        let object = &request.custody.object;
        let validated = self
            .db
            .query_row(
                "SELECT validated FROM sync_asset_manifests WHERE hub=?1 AND scope=?2 AND hash=?3",
                params![object.scope.hub.to_string(), object.scope.scope.as_str(), root.0.digest.as_str()],
                |row| row.get::<_, bool>(0),
            )
            .optional()?;
        if validated.is_some() {
            let reply = crate::blocks::pages::get(self.db, root)?;
            let page: ManifestPage = serde_json::from_str(&reply.page_json).map_err(|e| sql::invalid(e.to_string()))?;
            if !matches!(page, ManifestPage::Metadata { codec: MetadataCodec::TimelineV1, .. }) {
                return Err(sql::invalid("timeline fence root is not a validated timeline index page"));
            }
        }
        let bound:bool=self.db.query_row(
            "SELECT EXISTS(SELECT 1 FROM sync_timeline_heads WHERE hub=?1 AND scope=?2 AND conversation=?3 AND projection=?4 AND root=?5) OR EXISTS(SELECT 1 FROM sync_root_leases WHERE hub=?1 AND scope=?2 AND object=?3 AND kind=?6 AND hash=?5 AND root_kind='manifest' AND usage=?7 AND holding=?8 AND purpose=?9 AND released=0 AND expires>?10 AND supply_id IS NULL)",
            params![object.scope.hub.to_string(),object.scope.scope.as_str(),object.object.as_str(),projection.0.to_be_bytes().as_slice(),root.0.digest.as_str(),request.custody.kind.registration().id,model::encode(&request.usage)?,model::encode(&request.custody.holding)?,model::encode(&request.custody.purpose)?,self.now],|row|row.get(0),
        )?;
        if !bound {
            let waiting:bool=self.db.query_row(
                "SELECT EXISTS(SELECT 1 FROM sync_root_leases WHERE hub=?1 AND scope=?2 AND object=?3 AND kind=?4 AND hash=?5 AND root_kind='manifest' AND usage=?6 AND purpose=?7 AND released=0 AND expires>?8 AND supply_id IS NULL)",
                params![object.scope.hub.to_string(),object.scope.scope.as_str(),object.object.as_str(),request.custody.kind.registration().id,root.0.digest.as_str(),model::encode(&request.usage)?,model::encode(&request.custody.purpose)?,self.now],|row|row.get(0),
            )?;
            if waiting {
                return Err(fastctx_storage::Error::with_kind(
                    fastctx_storage::ErrorKind::Busy,
                    "timeline snapshot use is awaiting holding-period reconciliation",
                ));
            }
            let mismatched: bool = self.db.query_row(
                "SELECT EXISTS(SELECT 1 FROM sync_timeline_heads WHERE hub=?1 AND scope=?2 AND root=?3 AND (conversation<>?4 OR projection<>?5)) OR EXISTS(SELECT 1 FROM sync_root_leases WHERE hub=?1 AND scope=?2 AND hash=?3 AND root_kind='manifest' AND released=0 AND expires>?6 AND supply_id IS NULL AND json_extract(CAST(usage AS TEXT),'$.use')='timeline' AND (object<>?4 OR usage<>?7))",
                params![object.scope.hub.to_string(),object.scope.scope.as_str(),root.0.digest.as_str(),object.object.as_str(),projection.0.to_be_bytes().as_slice(),self.now,model::encode(&request.usage)?], |row| row.get(0),
            )?;
            if mismatched {
                return Err(sql::invalid("timeline root belongs to another conversation or projection"));
            }
            return Ok(None);
        }
        if validated != Some(true) {
            return Ok(Some(RootAcquired::Pending { completeness: Completeness::MetadataOnly }));
        }
        Ok(Some(RootAcquired::Held { lease: Box::new(root_holds::insert(self.db, operation, request)?) }))
    }
}
