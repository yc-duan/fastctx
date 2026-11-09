//! Discovery: a device pages through the scopes its accounts' directories
//! describe, each with its original proof and its feed's current cursor.
use super::{SqliteDirectory, rows::Described};
use crate::{
    cursor::ScopeCursor,
    identity::{AccountId, DeviceId, Digest, ScopeId, ScopeKey, ViewId},
    registry::{DataClass, DecisionState, REGISTRY, ScopePolicy},
    scope::{
        DiscoveryCursor, DiscoveryFence, ScopeDescriptor, ScopeDiscoveryRead, ScopeOffer, ScopeSubject, ScopesPage,
        account_scope,
    },
    store::{Error, FutureResult, feed, sql},
    transfer::{KindVersion, ViewDefinition},
};
use fastctx_storage::rusqlite::params;
use sha2::{Digest as _, Sha256};

/// Who is asking, as the transport built it from the authenticated connection:
/// the device and the accounts it serves on this hub.
#[derive(Clone, Debug)]
pub struct Recipient {
    pub device: DeviceId,
    pub accounts: Vec<AccountId>,
}

impl SqliteDirectory {
    /// One bounded page of the scopes the recipient's accounts describe, in
    /// scope order. The last page carries the directory feeds' cursors, from
    /// which the device follows later changes.
    pub fn discover(&self, recipient: Recipient, request: ScopeDiscoveryRead) -> FutureResult<'_, ScopesPage> {
        Box::pin(async move {
            let limit = fastctx_types::sync::PAGE_BYTES;
            if request.max_items == 0 || request.max_items > 256 || request.max_bytes == 0 || request.max_bytes > limit
            {
                return Err(Error::Invalid("discovery pages hold 1 to 256 scopes within one page budget".into()));
            }
            let hub = self.run.hub().clone();
            let mut directories: Vec<ScopeKey> = recipient
                .accounts
                .iter()
                .map(|account| ScopeKey { hub: hub.clone(), scope: account_scope(account) })
                .collect();
            directories.sort();
            directories.dedup();
            let view = view(&recipient.device, &directories);
            let (fence, after) = match request.cursor {
                Some(cursor) => {
                    if cursor.fence.recipient != recipient.device
                        || cursor.fence.view != view
                        || cursor.after.hub != hub
                    {
                        return Err(Error::Invalid("a discovery cursor belongs to another recipient or view".into()));
                    }
                    (Some(cursor.fence), Some(cursor.after.scope))
                }
                None => (None, None),
            };
            let (max_items, max_bytes) = (request.max_items as usize, request.max_bytes as usize);
            let device = recipient.device;
            self.blocks
                .database()
                .read(move |db| {
                    // One snapshot for the cursors and every row of the page.
                    let snapshot = db.unchecked_transaction()?;
                    let mut cursors = Vec::new();
                    for directory in &directories {
                        if let Some(mut cursor) = feed::cursor(&snapshot, directory)? {
                            if let Some(descriptor) = super::described(&snapshot, directory)? {
                                cursor.view = Some(feed_view(&descriptor).id().map_err(sql::invalid)?);
                            }
                            cursors.push(cursor);
                        }
                    }
                    let fence = fence.unwrap_or_else(|| DiscoveryFence {
                        recipient: device.clone(),
                        view: view.clone(),
                        snapshot: digest(&cursors),
                    });
                    let mut rows = Vec::new();
                    for directory in &directories {
                        let mut found = snapshot.prepare_cached("SELECT scope,origin FROM sync_scopes INDEXED BY sync_scopes_directory WHERE hub=?1 AND directory=?2 AND scope>?3 ORDER BY scope LIMIT ?4")?;
                        let page = found.query_map(
                            params![
                                hub.to_string(),
                                directory.scope.as_str(),
                                after.as_ref().map_or("", ScopeId::as_str),
                                max_items as i64 + 1
                            ],
                            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?)),
                        )?;
                        for row in page {
                            rows.push(row?);
                        }
                    }
                    rows.sort_by(|left, right| left.0.cmp(&right.0));
                    let (mut entries, mut bytes, mut more) = (Vec::new(), 0, false);
                    for (scope, origin) in rows {
                        if entries.len() == max_items {
                            more = true;
                            break;
                        }
                        let origin: Described =
                            serde_json::from_slice(&origin).map_err(|error| sql::invalid(error.to_string()))?;
                        let Described::Directory { proof } = origin else { continue };
                        let key = ScopeKey { hub: hub.clone(), scope: ScopeId::try_from(scope).map_err(sql::invalid)? };
                        let descriptor: ScopeDescriptor = serde_json::from_slice(&proof.payload.0)
                            .map_err(|error| sql::invalid(error.to_string()))?;
                        let mut cursor: ScopeCursor =
                            feed::cursor(&snapshot, &key)?.ok_or_else(|| sql::invalid("a described scope has no feed"))?;
                        let view = feed_view(&descriptor);
                        cursor.view = Some(view.id().map_err(sql::invalid)?);
                        let offer = ScopeOffer { binding: *proof, cursor, kinds: view.kinds };
                        let size = serde_json::to_vec(&offer).map_err(|error| sql::invalid(error.to_string()))?.len();
                        if bytes + size > max_bytes && !entries.is_empty() {
                            more = true;
                            break;
                        }
                        bytes += size;
                        entries.push(offer);
                    }
                    snapshot.commit()?;
                    let next = match entries.last() {
                        Some(last) if more => {
                            Some(DiscoveryCursor { fence: fence.clone(), after: last.cursor.scope.clone() })
                        }
                        _ => None,
                    };
                    let directories = if next.is_none() { cursors } else { Vec::new() };
                    Ok(ScopesPage { fence, entries, directories, next })
                })
                .await
                .map_err(|error| sql::error(error, self.blocks.worker().clone()))
        })
    }
}

/// Discovery and feed reads agree on the default complete v1 capability view.
pub(crate) fn feed_view(descriptor: &ScopeDescriptor) -> ViewDefinition {
    ViewDefinition {
        scope: descriptor.scope.clone(),
        acl_revision: descriptor.acl_revision,
        projection_schema: 1,
        kinds: offered(&descriptor.subject),
    }
}

/// The kinds a scope of this subject may carry, as the registry declares them.
fn offered(subject: &ScopeSubject) -> Vec<KindVersion> {
    let policy = match subject {
        ScopeSubject::Account => ScopePolicy::Account,
        ScopeSubject::Conversation { .. } => ScopePolicy::Conversation,
        ScopeSubject::Project { .. } => ScopePolicy::Project,
        ScopeSubject::Machine { .. } => ScopePolicy::Machine,
        ScopeSubject::Device { .. } => ScopePolicy::Device,
        ScopeSubject::Administrators => ScopePolicy::Administrators,
    };
    let mut kinds: Vec<_> = REGISTRY
        .entries()
        .iter()
        .filter(|entry| {
            entry.class == DataClass::Asset
                && entry.decision == DecisionState::Confirmed
                && entry.scopes.contains(&policy)
        })
        .map(|entry| KindVersion { kind: entry.id.into(), schema: entry.schema })
        .collect();
    kinds.sort();
    kinds
}

fn view(device: &DeviceId, directories: &[ScopeKey]) -> ViewId {
    let mut hash = Sha256::new();
    hash.update(b"fastctx.sync.discovery-view.v1\0");
    hash.update(device.as_str().as_bytes());
    for directory in directories {
        hash.update((directory.scope.as_str().len() as u64).to_be_bytes());
        hash.update(directory.scope.as_str().as_bytes());
    }
    ViewId::from_bytes(hash.finalize().into())
}

fn digest(cursors: &[ScopeCursor]) -> Digest {
    let mut hash = Sha256::new();
    hash.update(b"fastctx.sync.discovery-snapshot.v1\0");
    for cursor in cursors {
        hash.update(cursor.scope.scope.as_str().as_bytes());
        hash.update(cursor.epoch.as_str().as_bytes());
        hash.update(cursor.through.0.to_be_bytes());
    }
    Digest::from_bytes(hash.finalize().into())
}
