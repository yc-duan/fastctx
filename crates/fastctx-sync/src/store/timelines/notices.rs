//! Changes to the installed timelines of one scope, resumed strictly after a
//! fence's local sequence. The head rows are the durable record; the in-memory
//! wake only shortens the wait.
use super::SqliteTimelines;
use crate::{
    cursor::{LocalSequence, Revision},
    identity::{ObjectId, ObjectKey},
    store::{
        Error, FutureResult, PageLimit,
        access::AccessContext,
        projection::{ProjectionNotice, ProjectionSubscription},
        sql,
    },
};
use fastctx_storage::rusqlite::params;
use std::time::Duration;

/// How long one call waits for a change before returning empty.
const WAIT: Duration = Duration::from_secs(30);

pub struct TimelineNotices {
    timelines: SqliteTimelines,
    context: AccessContext,
    after: LocalSequence,
}
impl SqliteTimelines {
    /// Subscribe first, read a page second, then resume after its fence's local.
    pub fn subscribe(&self, context: AccessContext, after: LocalSequence) -> TimelineNotices {
        TimelineNotices { timelines: self.clone(), context, after }
    }
}
impl ProjectionSubscription for TimelineNotices {
    fn next(&mut self, limit: PageLimit) -> FutureResult<'_, Vec<ProjectionNotice>> {
        Box::pin(async move {
            if limit.items == 0 || limit.items > 256 {
                return Err(Error::Invalid("a notice page needs 1 to 256 entries".into()));
            }
            let woken = self.timelines.wake.clone();
            let mut waited = false;
            loop {
                // Register before reading, so a commit between the two still wakes us.
                let wake = woken.notified();
                let page = self.page(limit.items).await?;
                if let Some(last) = page.last() {
                    self.after = last.local;
                    let mut visible = Vec::with_capacity(page.len());
                    for notice in page {
                        let object = ObjectKey { scope: self.context.scope.clone(), object: notice.object.clone() };
                        if self.timelines.authorize(&self.context, &object).is_ok() {
                            visible.push(notice);
                        }
                    }
                    if !visible.is_empty() || waited {
                        return Ok(visible);
                    }
                    continue;
                }
                if waited {
                    return Ok(Vec::new());
                }
                let _ = tokio::time::timeout(WAIT, wake).await;
                waited = true;
            }
        })
    }
}
impl TimelineNotices {
    async fn page(&self, items: u32) -> Result<Vec<ProjectionNotice>, Error> {
        let scope = self.context.scope.clone();
        let after = self.after;
        self.timelines
            .blocks
            .database()
            .read(move |db| {
                db.prepare_cached("SELECT conversation,projection,applied,local FROM sync_timeline_heads WHERE hub=?1 AND scope=?2 AND local>?3 ORDER BY local LIMIT ?4")?
                    .query_map(
                        params![scope.hub.to_string(), scope.scope.as_str(), after.0.to_be_bytes().as_slice(), items],
                        |row| {
                            Ok((
                                row.get::<_, String>(0)?,
                                sql::counter(row, 1)?,
                                sql::counter(row, 2)?,
                                sql::counter(row, 3)?,
                            ))
                        },
                    )?
                    .map(|row| {
                        let (object, projection, applied, local) = row?;
                        Ok(ProjectionNotice {
                            object: ObjectId::try_from(object).map_err(sql::invalid)?,
                            revision: Revision(projection),
                            applied: LocalSequence(applied),
                            local: LocalSequence(local),
                        })
                    })
                    .collect()
            })
            .await
            .map_err(|error| sql::error(error, self.timelines.blocks.worker().clone()))
    }
}
