//! What the installed records say of a conversation's scope to one reader: the
//! checks every read makes, in the typed form whoever answers the reader needs
//! to say why it may not go on.
use super::SqliteTimelines;
use crate::{
    catalog::Lifecycle,
    cursor::{HoldingPeriod, Revision, VersionRef},
    identity::{AuthorityEpoch, ObjectKey},
    registry::REGISTRY,
    store::{
        Error,
        access::{AccessContext, Decision, Principal},
        scopes, sql,
    },
    timeline::{ScopeEnd, ScopeStanding},
};
use fastctx_storage::rusqlite::{OptionalExtension, params};
use fastctx_types::sync::conversation_timeline::KIND;
use std::time::Duration;

/// How long a reader that knows what the records say waits for them to change.
const WAIT: Duration = Duration::from_secs(30);

impl SqliteTimelines {
    /// None when no record of the scope is installed here, which says nothing
    /// of the scope itself. The row, the description on it and the object's
    /// tombstone are read at one moment. The policy then decides at the row's
    /// ACL revision; a decision it made at another revision is no answer yet.
    /// A reader the records refuse is told only that, not what became of the
    /// object.
    pub(super) async fn read_standing(
        &self,
        principal: Principal,
        conversation: ObjectKey,
    ) -> Result<Option<ScopeStanding>, Error> {
        let key = conversation.clone();
        let recorded = self
            .blocks
            .database()
            .read(move |db| {
                let Some(state) = scopes::current(db, &key.scope)? else { return Ok(None) };
                let lifecycle = scopes::lifecycle(db, &key.scope, state.enabled)?;
                let deleted = db
                    .prepare_cached(
                        "SELECT authority_epoch,revision FROM sync_tombstones WHERE hub=?1 AND scope=?2 AND object=?3",
                    )?
                    .query_row(
                        params![key.scope.hub.to_string(), key.scope.scope.as_str(), key.object.as_str()],
                        |row| Ok((row.get::<_, Vec<u8>>(0)?, sql::counter(row, 1)?)),
                    )
                    .optional()?;
                let deleted = deleted
                    .map(|(epoch, revision)| {
                        let epoch: [u8; 16] =
                            epoch.try_into().map_err(|_| sql::invalid("a tombstone's authority epoch is malformed"))?;
                        Ok::<_, fastctx_storage::Error>(VersionRef {
                            authority_epoch: AuthorityEpoch::from_bytes(epoch),
                            revision: Revision(revision),
                        })
                    })
                    .transpose()?;
                Ok(Some((state, lifecycle, deleted)))
            })
            .await
            .map_err(|error| sql::error(error, self.blocks.worker().clone()))?;
        let Some((state, lifecycle, deleted)) = recorded else { return Ok(None) };
        let period = HoldingPeriod {
            holder: self.blocks.holder().clone(),
            scope: conversation.scope.clone(),
            acl_revision: state.acl_revision,
            publication_epoch: state.publication_epoch,
        };
        if lifecycle != Lifecycle::Live {
            return Ok(Some(ScopeStanding { period, end: Some(ScopeEnd::Lifecycle { lifecycle }) }));
        }
        let context = AccessContext { principal, scope: conversation.scope.clone(), observed_acl: state.acl_revision };
        let kind = REGISTRY.asset(KIND).map_err(Error::Invalid)?.registered();
        let end = match self.blocks.policy().can_read_object(&context, kind, &conversation) {
            Decision::Allowed { revision } if revision == state.acl_revision => {
                deleted.map(|version| ScopeEnd::Deleted { version })
            }
            Decision::Denied { revision, reason } if revision == state.acl_revision => {
                Some(ScopeEnd::Refused { reason })
            }
            Decision::Allowed { .. } | Decision::Denied { .. } => return Err(Error::Busy),
            Decision::Unavailable => {
                return Err(Error::Unavailable("the reader's access cannot be decided now".into()));
            }
        };
        Ok(Some(ScopeStanding { period, end }))
    }

    /// The records are read again on each wake of the store, registered before
    /// the read so that a change between the two still ends the wait.
    pub(super) async fn wait_standing(
        &self,
        principal: Principal,
        conversation: ObjectKey,
        known: ScopeStanding,
    ) -> Result<Option<ScopeStanding>, Error> {
        let until = tokio::time::Instant::now() + WAIT;
        loop {
            let wake = self.wake.notified();
            match self.read_standing(principal.clone(), conversation.clone()).await? {
                Some(now) if now == known => {}
                other => return Ok(other),
            }
            if tokio::time::timeout_at(until, wake).await.is_err() {
                return Ok(Some(known));
            }
        }
    }
}
