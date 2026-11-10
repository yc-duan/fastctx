//! Passes: work the node makes over everything of one sort it keeps, such
//! as every receipt still to forward, again and again while a pass leaves
//! some undone. Each sort of pass is one job (`fastctx_jobs`): asked for
//! while it waits to be made again, it is made now; asked for while it is
//! made, another follows. What a pass looks at stays where it is kept.

use crate::Node;
use fastctx_jobs::{Job, Kind, Outcome, Taken};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

/// One sort of pass and how it is paced.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Pass {
    /// The pass's name, which is its job kind.
    pub name: &'static str,
    /// The longest one pass may take.
    pub within: Duration,
    /// How long after a pass that left nothing the next is made unasked;
    /// none where a pass is made only when asked for.
    pub rests: Option<Duration>,
}

/// What a pass left.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Left {
    /// Nothing: the next pass is made when asked for, or after its rest.
    Nothing,
    /// Work it could not do now: it is made again at the pace of everything
    /// that is tried again (`fastctx_net::again`).
    Work,
}

/// Asks for a pass now.
pub(crate) fn ask(node: &Node, name: &'static str) {
    let job = Job { kind: name, key: "pass".into(), unit: "node".into(), payload: Vec::new() };
    let (database, jobs) = (node.database.clone(), node.durable_jobs.clone());
    fastctx_platform::task::spawn(async move {
        if database.write_relaxed(128, move |db| fastctx_jobs::submit(db, &job)).await.is_ok() {
            jobs.stir(name);
        }
    });
}

/// Names what makes a sort of pass, before the node's jobs start, and asks
/// for the first.
pub(crate) fn register<F, Making>(node: &Arc<Node>, pass: Pass, make: F)
where
    F: Fn(Arc<Node>) -> Making + Send + Sync + 'static,
    Making: Future<Output = Left> + Send + 'static,
{
    let weak = Arc::downgrade(node);
    let (make, failures) = (Arc::new(make), Arc::new(AtomicU32::new(0)));
    let run = move |_: Taken| -> Pin<Box<dyn Future<Output = Outcome> + Send>> {
        let (node, make, failures) = (weak.upgrade(), make.clone(), failures.clone());
        Box::pin(async move {
            let Some(node) = node else { return Outcome::Failed("the node is stopping".into()) };
            match make(node).await {
                Left::Work => {
                    let failed = failures.fetch_add(1, Ordering::AcqRel).saturating_add(1);
                    Outcome::Again { after: fastctx_net::again::backoff(failed) }
                }
                Left::Nothing => {
                    failures.store(0, Ordering::Release);
                    match pass.rests {
                        Some(after) => Outcome::Again { after },
                        None => Outcome::Done,
                    }
                }
            }
        })
    };
    node.durable_jobs.register(Kind { name: pass.name, at_once: 1, within: pass.within, tries: 5 }, Arc::new(run));
    ask(node, pass.name);
}

/// Has each change of `changes` ask for a pass, once the changes have been
/// quiet for `settles`: what changes in bursts, as the ways to other
/// machines do, asks once.
pub(crate) fn follow(
    node: &Arc<Node>,
    name: &'static str,
    mut changes: tokio::sync::watch::Receiver<u64>,
    settles: Duration,
) {
    let weak = Arc::downgrade(node);
    fastctx_platform::task::spawn(async move {
        while changes.changed().await.is_ok() {
            tokio::time::sleep(settles).await;
            changes.borrow_and_update();
            let Some(node) = weak.upgrade() else { return };
            ask(&node, name);
        }
    });
}
