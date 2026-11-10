//! Direct grants are an object-specific closure cache, never a staging pin.
use super::*;
use std::collections::HashSet;

impl Service {
    pub(super) async fn member(&self, custody: &Custody, root: &ContentRef, node: &ContentRef) -> Result<(), Error> {
        let blocks = self.supply_blocks()?;
        let (object, kind, reference, granted_root) =
            (custody.object.clone(), custody.kind.registration().id, node.clone(), root.clone());
        let granted = blocks
            .database()
            .read(move |db| {
                let (form, length) = shape(&reference);
                let direct = granted_root == reference
                    || (matches!(granted_root, ContentRef::Manifest { .. })
                        && db.query_row(
                            "SELECT EXISTS(SELECT 1 FROM sync_asset_edges e
                         JOIN sync_asset_manifests m ON m.hub=e.hub AND m.scope=e.scope AND m.hash=e.parent
                         WHERE e.hub=?1 AND e.scope=?2 AND e.parent=?3 AND e.child=?4
                           AND e.child_kind=?5 AND e.decoded_bytes IS ?6 AND m.validated=1)",
                            params![
                                object.scope.hub.to_string(),
                                object.scope.scope.as_str(),
                                granted_root.id().digest.as_str(),
                                reference.id().digest.as_str(),
                                form,
                                length
                            ],
                            |row| row.get::<_, bool>(0),
                        )?);
                Ok(direct
                    && db.query_row(
                        "SELECT EXISTS(SELECT 1 FROM sync_asset_access a
                 WHERE a.hub=?1 AND a.scope=?2 AND a.object=?3 AND a.kind=?4
                   AND a.hash=?5 AND a.root_kind=?6 AND a.decoded_bytes IS ?7
                   AND NOT EXISTS(SELECT 1 FROM sync_tombstones t
                                  WHERE t.hub=a.hub AND t.scope=a.scope AND t.object=a.object))",
                        params![
                            object.scope.hub.to_string(),
                            object.scope.scope.as_str(),
                            object.object.as_str(),
                            kind,
                            reference.id().digest.as_str(),
                            form,
                            length
                        ],
                        |row| row.get::<_, bool>(0),
                    )?)
            })
            .await
            .map_err(|error| read::ReadError::from(error).0)?;
        if granted {
            return Ok(());
        }
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
        // Retry with a larger complete reservation only after dropping the old
        // walk. Holding bytes while joining the FIFO for growth can self-lock.
        let mut nodes = 8usize;
        loop {
            let bytes = nodes.checked_mul(512).ok_or(Error::Busy)?;
            let budget = blocks.buffer_budget();
            if bytes > budget.capacity() {
                return Err(Error::Unavailable("the root's reachability proof exceeds the runtime workspace".into()));
            }
            let permit = budget.acquire_until(bytes, deadline).await?;
            let (scope, root, node) = (custody.object.scope.clone(), root.clone(), node.clone());
            let found = blocks
                .database()
                .read(move |db| {
                    let _permit = permit;
                    walk(db, &scope, &root, &node, nodes)
                })
                .await
                .map_err(|error| read::ReadError::from(error).0)??;

            match found {
                Some(true) => return Ok(()),
                Some(false) => return Err(Error::Denied("block is not in the granted root's closure".into())),
                None => nodes = nodes.checked_mul(2).ok_or(Error::Busy)?,
            }
        }
    }
}
fn shape(reference: &ContentRef) -> (&'static str, Option<u32>) {
    match reference {
        ContentRef::Single { decoded_bytes, .. } => ("block", Some(*decoded_bytes)),
        ContentRef::Manifest { .. } => ("manifest", None),
    }
}
fn walk(
    db: &Connection,
    scope: &ScopeKey,
    root: &ContentRef,
    node: &ContentRef,
    max: usize,
) -> fastctx_storage::Result<Result<Option<bool>, Error>> {
    let (form, length) = shape(root);
    let wanted = (node.id().digest.as_str(), shape(node));
    let mut pending = vec![(root.id().digest.as_str().to_owned(), form.to_owned(), length)];
    let mut seen = HashSet::new();
    let mut query = db.prepare_cached(
        "SELECT child,child_kind,decoded_bytes FROM sync_asset_edges
         WHERE hub=?1 AND scope=?2 AND parent=?3 AND child_kind='manifest'
         ORDER BY child,child_kind LIMIT ?4",
    )?;
    while let Some((hash, form, length)) = pending.pop() {
        if seen.contains(&(hash.clone(), form.clone())) {
            continue;
        }
        if seen.len() + pending.len() >= max {
            return Ok(Ok(None));
        }
        seen.insert((hash.clone(), form.clone()));
        if hash == wanted.0 && (form.as_str(), length) == wanted.1 {
            return Ok(Ok(Some(true)));
        }
        if form != "manifest" {
            continue;
        }
        let present: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM sync_asset_manifests
             WHERE hub=?1 AND scope=?2 AND hash=?3 AND validated=1)",
            params![scope.hub.to_string(), scope.scope.as_str(), hash],
            |row| row.get(0),
        )?;
        if !present {
            return Ok(Err(Error::Unavailable("a granted closure page is not stored".into())));
        }
        let direct: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM sync_asset_edges
             WHERE hub=?1 AND scope=?2 AND parent=?3 AND child=?4 AND child_kind=?5 AND decoded_bytes IS ?6)",
            params![scope.hub.to_string(), scope.scope.as_str(), hash, wanted.0, wanted.1.0, wanted.1.1],
            |row| row.get(0),
        )?;
        if direct {
            return Ok(Ok(Some(true)));
        }
        let mut rows = query.query(params![scope.hub.to_string(), scope.scope.as_str(), hash, max as i64 + 1])?;
        while let Some(row) = rows.next()? {
            if seen.len() + pending.len() >= max {
                return Ok(Ok(None));
            }
            pending.push((row.get(0)?, row.get(1)?, row.get(2)?));
        }
    }
    Ok(Ok(Some(false)))
}
