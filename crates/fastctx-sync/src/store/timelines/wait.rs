//! A conversation-specific long poll over the durable head-notice subscription.
use super::SqliteTimelines;
use crate::{
    cursor::LocalSequence,
    identity::ObjectKey,
    store::{Error, PageLimit, access::AccessContext, projection::ProjectionSubscription},
    timeline::TimelineWait,
};
use std::time::Duration;
impl SqliteTimelines {
    pub(super) async fn wait_head(
        &self,
        context: AccessContext,
        conversation: ObjectKey,
        after: LocalSequence,
    ) -> Result<TimelineWait, Error> {
        self.authorize(&context, &conversation)?;
        let mut notices = self.subscribe(context.clone(), after);
        let until = tokio::time::Instant::now() + Duration::from_secs(30);
        let result = loop {
            let page = tokio::time::timeout_at(
                until,
                notices.next(PageLimit { items: 256, bytes: fastctx_types::sync::PAGE_BYTES }),
            )
            .await;
            match page {
                Ok(Ok(page)) => {
                    if let Some(notice) = page.into_iter().find(|notice| notice.object == conversation.object) {
                        break TimelineWait::Moved { local: notice.local };
                    }
                    if tokio::time::Instant::now() >= until {
                        break TimelineWait::Unchanged;
                    }
                }
                Ok(Err(error)) => return Err(error),
                _ => break TimelineWait::Unchanged,
            }
        };
        self.authorize(&context, &conversation)?;
        Ok(result)
    }
}
