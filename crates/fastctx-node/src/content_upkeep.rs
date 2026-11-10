//! Keeping the content database: objects nothing refers to are reclaimed
//! (`fastctx_content::Content::reclaim`).
//!
//! It is one job that never ends: a bounded pass, then the next at once
//! while there is more to look at, and after a rest when there is not.

use fastctx_jobs::{Job, Kind, Outcome, Taken};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

const KIND: &str = "content/reclaim";
/// How long ago an object was put before it may go: a record is appended
/// within this of putting what it refers to.
const GRACE: Duration = Duration::from_secs(24 * 60 * 60);
/// The rest between rounds over all objects.
const RESTS: Duration = Duration::from_secs(60 * 60);
/// The longest one pass may take.
const WITHIN: Duration = Duration::from_secs(10 * 60);

/// Names what reclaims objects and asks for it, before the node's jobs start.
pub(crate) fn register(node: &Arc<crate::Node>) {
    let jobs = node.durable_jobs.clone();
    let database = node.database.clone();
    let node = Arc::downgrade(node);
    let run = move |_: Taken| -> Pin<Box<dyn Future<Output = Outcome> + Send>> {
        let node = node.upgrade();
        Box::pin(async move {
            let Some(node) = node else { return Outcome::Failed("the node is stopping".into()) };
            // A machine whose content database did not open has nothing to keep.
            let Ok(content) = node.content().cloned() else { return Outcome::Again { after: RESTS } };
            let pass = fastctx_platform::task::spawn_blocking(move || content.reclaim(GRACE)).await;
            match pass {
                Ok(Ok(pass)) => {
                    if pass.removed > 0 {
                        node.log.line(format_args!(
                            "content: reclaimed {} objects, {} bytes, of {} looked at",
                            pass.removed, pass.bytes, pass.looked
                        ));
                    }
                    Outcome::Again { after: if pass.more { Duration::ZERO } else { RESTS } }
                }
                Ok(Err(error)) => Outcome::Failed(error.to_string()),
                Err(error) => Outcome::Failed(error.to_string()),
            }
        })
    };
    jobs.register(Kind { name: KIND, at_once: 1, within: WITHIN, tries: 5 }, Arc::new(run));
    // Asked for at each start: one that was set aside after failing is
    // tried again, and one that waits is the same job.
    fastctx_platform::task::spawn(async move {
        let job = Job { kind: KIND, key: "objects".into(), unit: "content".into(), payload: Vec::new() };
        let _ = database.write_relaxed(256, move |db| fastctx_jobs::submit(db, &job)).await;
    });
}
