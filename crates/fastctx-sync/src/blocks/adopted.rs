//! A source commit leaves what it adopted readable before its receipt returns:
//! the granted manifests are expanded down their closure in short transactions.
//! The walk follows only pages not yet expanded, so content an earlier commit
//! already opened costs nothing; whatever the budget leaves behind, the content
//! worker's queued expansion finishes.
use super::{SqliteBlocks, access, manifest::Reference, pages};
use crate::{
    cursor::HoldingPeriod,
    identity::ObjectKey,
    manifest::{ContentRef, ManifestRoot},
    store::{
        Error,
        access::Custody,
        boundary::{self, Role},
        sql,
    },
};
use fastctx_storage::rusqlite::{OptionalExtension, params};

/// Pages one transaction expands.
const PAGE: usize = 64;
/// Pages one commit expands before leaving the rest to the content worker.
const BUDGET: usize = 4096;

#[derive(Clone)]
struct Pending {
    object: ObjectKey,
    kind: String,
    holding: HoldingPeriod,
    root: ManifestRoot,
}

impl SqliteBlocks {
    pub(crate) async fn expand_adopted(&self, adopted: &[(Custody, ContentRef)]) -> Result<(), Error> {
        let mut frontier: Vec<Pending> = adopted
            .iter()
            .filter_map(|(custody, root)| match root {
                ContentRef::Manifest { root } => Some(Pending {
                    object: custody.object.clone(),
                    kind: custody.kind.registration().id.into(),
                    holding: custody.holding.clone(),
                    root: root.clone(),
                }),
                ContentRef::Single { .. } => None,
            })
            .collect();
        let mut spent = 0;
        while !frontier.is_empty() && spent < BUDGET {
            let take = frontier.len().min(PAGE).min(BUDGET - spent);
            let page: Vec<Pending> = frontier.drain(..take).collect();
            spent += page.len();
            let next = self
                .database()
                .write(PAGE * 8192, move |db| {
                    let _boundary = boundary::enter_typed_write(Role::Content);
                    let mut next = Vec::new();
                    for pending in page {
                        let scope = &pending.object.scope;
                        let expanded: Option<bool> = db
                            .prepare_cached("SELECT expanded FROM sync_asset_access WHERE hub=?1 AND scope=?2 AND object=?3 AND kind=?4 AND hash=?5 AND root_kind='manifest'")?
                            .query_row(
                                params![
                                    scope.hub.to_string(),
                                    scope.scope.as_str(),
                                    pending.object.object.as_str(),
                                    pending.kind,
                                    pending.root.0.digest.as_str()
                                ],
                                |row| row.get(0),
                            )
                            .optional()?;
                        let Some(expanded) = expanded else {
                            continue;
                        };
                        let children = pages::references(db, scope, &pending.root.0.digest)?;
                        if !expanded {
                            let references: Vec<_> = children
                                .iter()
                                .map(|child| match child {
                                    ContentRef::Single { block, decoded_bytes } => {
                                        Reference::Block { id: block.clone(), decoded_bytes: *decoded_bytes }
                                    }
                                    ContentRef::Manifest { root } => Reference::Manifest { root: root.clone() },
                                })
                                .collect();
                            access::expand(db, &pending.object, &pending.kind, &pending.root, &references, &pending.holding)?;
                        }
                        // An expanded page's children are granted; those not yet
                        // expanded themselves still need the walk.
                        for child in children {
                            let ContentRef::Manifest { root } = child else { continue };
                            let open: Option<bool> = db
                                .prepare_cached("SELECT expanded FROM sync_asset_access WHERE hub=?1 AND scope=?2 AND object=?3 AND kind=?4 AND hash=?5 AND root_kind='manifest'")?
                                .query_row(
                                    params![
                                        scope.hub.to_string(),
                                        scope.scope.as_str(),
                                        pending.object.object.as_str(),
                                        pending.kind,
                                        root.0.digest.as_str()
                                    ],
                                    |row| row.get(0),
                                )
                                .optional()?;
                            if open == Some(false) {
                                next.push(Pending { root, ..pending.clone() });
                            }
                        }
                    }
                    Ok(next)
                })
                .await
                .map_err(|error| sql::error(error, self.worker().clone()))?;
            frontier.extend(next);
        }
        Ok(())
    }
}
