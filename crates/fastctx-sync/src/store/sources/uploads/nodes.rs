//! Closure work is deduplicated by typed content identity and committed per node.
use super::*;
use crate::{
    identity::Digest,
    manifest::{BlobId, ContentBlock, ContentRef, DigestAlgorithm, ManifestReply, ManifestRoot},
};
use fastctx_storage::rusqlite::params;
pub(crate) struct UploadNode {
    pub root: ContentRef,
    pub expanded: bool,
}
impl SourceUploads {
    pub(crate) async fn next_nodes(&self, state: &SourceUploadState, max: usize) -> Result<Vec<UploadNode>, Error> {
        self.check(state).await?;
        let expected = state.clone();
        self.sources.blocks.database().read(move|db| {
            let k=&expected.key;
            let rows=db.prepare_cached("SELECT n.hash,n.form,n.decoded_bytes,n.expanded FROM sync_source_upload_nodes n WHERE n.hub=?1 AND n.scope=?2 AND n.stream=?3 AND n.epoch=?4 AND n.proof=?5 AND n.done=0 AND (n.form='block' OR n.expanded=0 OR NOT EXISTS(SELECT 1 FROM sync_asset_edges e LEFT JOIN sync_source_upload_nodes c ON c.hub=n.hub AND c.scope=n.scope AND c.stream=n.stream AND c.epoch=n.epoch AND c.proof=n.proof AND c.hash=e.child AND c.form=e.child_kind WHERE e.hub=n.hub AND e.scope=n.scope AND e.parent=n.hash AND (c.done IS NULL OR c.done=0))) ORDER BY (n.form='block') DESC,n.expanded,n.hash LIMIT ?6")?
                .query_map(params![k.scope.hub.to_string(),k.scope.scope.as_str(),k.stream.as_str(),k.epoch.as_str(),expected.proof.as_ref().map(Digest::as_str),max as u32],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,Option<u32>>(2)?,r.get::<_,bool>(3)?)))?.collect::<fastctx_storage::rusqlite::Result<Vec<_>>>()?;
            let mut nodes=Vec::with_capacity(rows.len());
            for (hash,form,length,expanded) in rows {
                let id=BlobId{domain:k.scope.clone(),algorithm:DigestAlgorithm::Sha256,digest:Digest::try_from(hash).map_err(sql::invalid)?};
                let root=if form=="manifest" {ContentRef::Manifest{root:ManifestRoot(id)}} else {ContentRef::Single{block:id,decoded_bytes:length.ok_or_else(||sql::invalid("source block has no decoded length"))?}};
                nodes.push(UploadNode{root,expanded});
            }
            if !nodes.is_empty() {return Ok(nodes);}
            let pending:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM sync_source_upload_nodes WHERE hub=?1 AND scope=?2 AND stream=?3 AND epoch=?4 AND proof=?5 AND done=0)",params![k.scope.hub.to_string(),k.scope.scope.as_str(),k.stream.as_str(),k.epoch.as_str(),expected.proof.as_ref().map(Digest::as_str)],|r|r.get(0))?;
            if pending {return Err(sql::invalid("source closure has unresolved dependency work"));}
            Ok(Vec::new())
        }).await.map_err(|e|sql::error(e,self.sources.blocks.worker().clone()))
    }
    pub(crate) async fn expand(
        &self,
        state: &SourceUploadState,
        node: UploadNode,
        page: ManifestReply,
    ) -> Result<(), Error> {
        let checked = crate::blocks::manifest::validate(&state.key.scope, page)?;
        let roots: Vec<_> = checked
            .references()
            .iter()
            .map(|r| match r {
                crate::blocks::manifest::Reference::Block { id, decoded_bytes } => {
                    ContentRef::Single { block: id.clone(), decoded_bytes: *decoded_bytes }
                }
                crate::blocks::manifest::Reference::Manifest { root } => ContentRef::Manifest { root: root.clone() },
            })
            .collect();
        let expected = state.clone();
        self.change(state,fastctx_types::sync::PAGE_BYTES as usize*4,move|db|{
            for root in roots {insert(db,&expected,&root)?;}
            let k=&expected.key;
            db.execute("UPDATE sync_source_upload_nodes SET expanded=1 WHERE hub=?1 AND scope=?2 AND stream=?3 AND epoch=?4 AND proof=?5 AND hash=?6 AND form='manifest'",
                params![k.scope.hub.to_string(),k.scope.scope.as_str(),k.stream.as_str(),k.epoch.as_str(),expected.proof.as_ref().map(Digest::as_str),node.root.id().digest.as_str()])?;Ok(())
        }).await
    }
    pub(crate) async fn node_done(&self, state: &SourceUploadState, node: UploadNode) -> Result<(), Error> {
        let expected = state.clone();
        let form = if matches!(node.root, ContentRef::Manifest { .. }) { "manifest" } else { "block" };
        self.change(state,8192,move|db|{
            let k=&expected.key;
            db.execute("UPDATE sync_source_upload_nodes SET done=1 WHERE hub=?1 AND scope=?2 AND stream=?3 AND epoch=?4 AND proof=?5 AND hash=?6 AND form=?7",
                params![k.scope.hub.to_string(),k.scope.scope.as_str(),k.stream.as_str(),k.epoch.as_str(),expected.proof.as_ref().map(Digest::as_str),node.root.id().digest.as_str(),form])?;Ok(())
        }).await
    }
    pub(crate) async fn ready(&self, state: &SourceUploadState) -> Result<(), Error> {
        let mut next = state.clone();
        next.phase = SourceUploadPhase::Submit;
        self.change(state, 8192, move |db| rows::save(db, &next)).await
    }
    pub(crate) async fn page(
        &self,
        state: &SourceUploadState,
        root: &ManifestRoot,
    ) -> Result<(ManifestReply, crate::blocks::Permit), Error> {
        self.check(state).await?;
        let page = self.sources.blocks.source_upload_page(root)?;
        self.check(state).await?;
        Ok(page)
    }
    pub(crate) async fn block(
        &self,
        state: &SourceUploadState,
        block: &ContentBlock,
    ) -> Result<(Vec<u8>, crate::blocks::Permit), Error> {
        self.check(state).await?;
        let bytes = self
            .sources
            .blocks
            .stored_block(block)?
            .ok_or_else(|| Error::Unavailable("adopted source block is missing locally".into()))?;
        self.check(state).await?;
        Ok(bytes)
    }
    pub(crate) async fn prune(&self, state: &SourceUploadState) -> Result<(), Error> {
        let expected = state.clone();
        self.change(state,16384,move|db|{
            let k=&expected.key;
            db.execute("DELETE FROM sync_source_upload_nodes WHERE (hub,scope,stream,epoch,proof,hash,form) IN (SELECT hub,scope,stream,epoch,proof,hash,form FROM sync_source_upload_nodes WHERE hub=?1 AND scope=?2 AND stream=?3 AND epoch=?4 AND NOT EXISTS(SELECT 1 FROM sync_source_upload_batches b WHERE b.hub=sync_source_upload_nodes.hub AND b.scope=sync_source_upload_nodes.scope AND b.stream=sync_source_upload_nodes.stream AND b.epoch=sync_source_upload_nodes.epoch AND b.proof=sync_source_upload_nodes.proof) ORDER BY proof,hash,form LIMIT 64)",params![k.scope.hub.to_string(),k.scope.scope.as_str(),k.stream.as_str(),k.epoch.as_str()])?;Ok(())
        }).await
    }
}
pub(super) fn insert(db: &Connection, state: &SourceUploadState, root: &ContentRef) -> SqlResult<()> {
    root.validate().map_err(sql::invalid)?;
    if root.id().domain != state.key.scope {
        return Err(sql::invalid("source closure crosses its scope"));
    }
    let (form, length, expanded) = match root {
        ContentRef::Single { decoded_bytes, .. } => ("block", Some(*decoded_bytes), true),
        ContentRef::Manifest { .. } => ("manifest", None, false),
    };
    let k = &state.key;
    db.execute("INSERT INTO sync_source_upload_nodes(hub,scope,stream,epoch,proof,hash,form,decoded_bytes,expanded,done) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,0) ON CONFLICT DO NOTHING",params![k.scope.hub.to_string(),k.scope.scope.as_str(),k.stream.as_str(),k.epoch.as_str(),state.proof.as_ref().map(Digest::as_str),root.id().digest.as_str(),form,length,expanded])?;
    let actual:Option<u32>=db.query_row("SELECT decoded_bytes FROM sync_source_upload_nodes WHERE hub=?1 AND scope=?2 AND stream=?3 AND epoch=?4 AND proof=?5 AND hash=?6 AND form=?7",params![k.scope.hub.to_string(),k.scope.scope.as_str(),k.stream.as_str(),k.epoch.as_str(),state.proof.as_ref().map(Digest::as_str),root.id().digest.as_str(),form],|r|r.get(0))?;
    if actual != length {
        return Err(sql::invalid("source closure has conflicting decoded lengths"));
    }
    Ok(())
}
