//! Executes a pure two-snapshot diff while retaining both exact root uses.
use super::{
    SqliteTimelines,
    pages::Loaded,
    reader::{Fresh, READ_BUDGET},
};
use crate::{
    store::{Error, PageLimit, access::AccessContext},
    timeline::{TimelineChangesRead, TimelineChangesResult, changes, pages::Outcome},
};
impl SqliteTimelines {
    pub(super) async fn read_changes(
        &self,
        context: AccessContext,
        request: TimelineChangesRead,
    ) -> Result<TimelineChangesResult, Error> {
        let conversation = &request.from.conversation;
        self.authorize(&context, conversation)?;
        if !self.held(&request.from).await? {
            return Ok(TimelineChangesResult::ReanchorNeeded);
        }
        let fresh = request.to.is_none();
        let to = match request.to {
            Some(to) if to.conversation != *conversation => {
                return Err(Error::Invalid("diff fences name different conversations".into()));
            }
            Some(to) => to,
            None => match self.fresh(conversation).await? {
                Fresh::Fence(to) => to,
                Fresh::Pending(progress) => return Ok(TimelineChangesResult::Pending { progress }),
            },
        };
        let outcome = async {
            if !self.held(&to).await? {
                return Ok(TimelineChangesResult::ReanchorNeeded);
            }
            let mut loaded = Loaded::default();
            let mut limit = PageLimit { items: request.max_items, bytes: request.max_bytes };
            let result = loop {
                match changes::page(&loaded, &request.from, &to, request.after.as_ref(), limit, READ_BUDGET) {
                    Ok(Outcome::Ready(page)) => {
                        break TimelineChangesResult::Page {
                            to: Box::new(to.clone()),
                            changes: page.changes,
                            next: page.next,
                        };
                    }
                    Ok(Outcome::Pending { missing }) => match loaded.load(&self.blocks, &missing) {
                        Ok(()) => {}
                        Err(Error::Unavailable(_)) => {
                            break TimelineChangesResult::Pending { progress: Some(to.through.clone()) };
                        }
                        Err(error) => return Err(error),
                    },
                    Err(Error::Busy) if limit.items > 1 => limit.items = (limit.items / 2).max(1),
                    Err(error) => return Err(error),
                }
            };
            self.authorize(&context, conversation)?;
            if !self.held(&request.from).await? || !self.held(&to).await? {
                return Ok(TimelineChangesResult::ReanchorNeeded);
            }
            Ok(result)
        }
        .await;
        // A retry must not accumulate undisclosed holds when the target is partial.
        if fresh && !matches!(outcome, Ok(TimelineChangesResult::Page { .. })) {
            self.release(context, to).await?;
        }
        outcome
    }
}
