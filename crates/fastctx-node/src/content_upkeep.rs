//! Keeping the content database: objects nothing refers to are reclaimed
//! (`fastctx_content::Content::reclaim`).
//!
//! It is one job that never ends: a bounded pass, then the next at once
//! while there is more to look at, and after a rest when there is not.
//!
//! A conversation that is deleted is forgotten here too
//! (`fastctx_content::Content::forget`): the deletion asks for it in its own
//! transaction, so the one is not there without the other.

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

const FORGETTING: &str = "content/forget";
/// The longest forgetting one conversation may take in one run.
const FORGETS_WITHIN: Duration = Duration::from_secs(10 * 60);
/// How long after a run that removed something the conversation is looked
/// at once more: a build that was writing as the deletion began may have
/// written after it.
const LOOKS_AGAIN: Duration = Duration::from_secs(30);

/// Asks, in the transaction that begins a conversation's deletion, for what
/// the content database keeps of it to go.
pub(crate) fn forget_in(db: &fastctx_storage::rusqlite::Connection, conversation: &str) -> fastctx_storage::Result<()> {
    let job = Job {
        kind: FORGETTING,
        key: conversation.to_string(),
        // Not the conversation's own unit, whose jobs its deletion drops.
        unit: format!("content/{conversation}"),
        payload: Vec::new(),
    };
    fastctx_jobs::submit(db, &job)
}

/// A deletion began: what it asked to be forgotten is looked at now.
pub(crate) fn forgetting(node: &crate::Node) {
    node.durable_jobs.stir(FORGETTING);
}

/// Forgets a deleted conversation until nothing of it is left; says whether
/// anything was removed.
fn forget(content: &fastctx_content::Content, conversation: &str) -> fastctx_content::Result<bool> {
    let scope = crate::timeline::scope(conversation);
    let mut removed = false;
    loop {
        let pass = content.forget(&scope)?;
        removed |= pass.more || pass.items + pass.frames + pass.objects > 0;
        if !pass.more {
            break;
        }
    }
    content.drop_checkpoint(&crate::timeline::checkpoint(conversation))?;
    Ok(removed)
}

/// Names what reclaims objects and asks for it, and what forgets deleted
/// conversations, before the node's jobs start.
pub(crate) fn register(node: &Arc<crate::Node>) {
    let forgetting = Arc::downgrade(node);
    let run = move |taken: Taken| -> Pin<Box<dyn Future<Output = Outcome> + Send>> {
        let node = forgetting.upgrade();
        Box::pin(async move {
            let Some(node) = node else { return Outcome::Failed("the node is stopping".into()) };
            let Ok(content) = node.content().cloned() else { return Outcome::Done };
            let conversation = taken.key;
            match fastctx_platform::task::spawn_blocking(move || forget(&content, &conversation)).await {
                Ok(Ok(true)) => Outcome::Again { after: LOOKS_AGAIN },
                Ok(Ok(false)) => Outcome::Done,
                Ok(Err(error)) => Outcome::Failed(error.to_string()),
                Err(error) => Outcome::Failed(error.to_string()),
            }
        })
    };
    node.durable_jobs.register(Kind { name: FORGETTING, at_once: 2, within: FORGETS_WITHIN, tries: 5 }, Arc::new(run));
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
