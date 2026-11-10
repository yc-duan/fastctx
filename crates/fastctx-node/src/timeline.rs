//! A conversation's timeline as the content database keeps it: each event
//! of its history an item, so a viewer reads a page of it and is told when
//! it moves, where it once received the whole history.
//!
//! The items are the session's events as interfaces already draw them. They
//! are made by `build`, which reads the history as the World and the host
//! keep it and writes what changed.

pub(crate) mod api;
mod build;
mod identity;
mod order;
pub(crate) mod reads;

use fastctx_content::StreamKey;
use fastctx_jobs::{Job, Kind, Outcome, Taken};
use fastctx_types::ResidentId;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

/// The kind of job that brings a conversation's timeline to its history.
const KIND: &str = "timeline";
/// A build reads a conversation's whole history: few run at once.
const AT_ONCE: usize = 2;
/// The longest one build may take.
const WITHIN: Duration = Duration::from_secs(600);
/// Builds in a row that may fail before the job is set aside until asked again.
const TRIES: u32 = 5;
/// The soonest a conversation's timeline is built again after it was built:
/// while a turn runs its history changes with every item, and each build
/// reads all of it.
const APART: Duration = Duration::from_secs(15);

/// When each conversation's timeline was last built, in this run of the node.
static BUILT: LazyLock<Mutex<HashMap<ResidentId, Instant>>> = LazyLock::new(Mutex::default);

/// The stream a conversation's timeline is kept in.
pub(crate) fn stream(resident: &ResidentId) -> StreamKey {
    StreamKey::new(format!("conversation/{resident}"), "timeline")
}

/// Names what builds timelines, before the node's jobs start.
pub(crate) fn register(node: &Arc<crate::Node>) {
    let jobs = node.durable_jobs.clone();
    let node = Arc::downgrade(node);
    let run = move |taken: Taken| -> std::pin::Pin<Box<dyn Future<Output = Outcome> + Send>> {
        let node = node.upgrade();
        Box::pin(async move {
            let Some(node) = node else { return Outcome::Failed("the node is stopping".into()) };
            match taken.unit.parse::<ResidentId>() {
                Ok(resident) => run(&node, &resident).await,
                Err(_) => Outcome::Done,
            }
        })
    };
    jobs.register(Kind { name: KIND, at_once: AT_ONCE, within: WITHIN, tries: TRIES }, Arc::new(run));
}

async fn run(node: &Arc<crate::Node>, resident: &ResidentId) -> Outcome {
    if node.residents.about(resident).is_none() {
        return Outcome::Done;
    }
    let since = BUILT.lock().expect("built timelines").get(resident).map(Instant::elapsed);
    if let Some(since) = since
        && since < APART
    {
        return Outcome::Again { after: APART - since };
    }
    match build::build(node, resident).await {
        Ok(built) => {
            BUILT.lock().expect("built timelines").insert(resident.clone(), Instant::now());
            if built.written + built.gone > 0 {
                node.log.line(format_args!(
                    "{resident}'s timeline: {} events, {} written, {} gone",
                    built.events, built.written, built.gone
                ));
            }
            Outcome::Done
        }
        Err(failure) => Outcome::Failed(failure.message),
    }
}

/// Asks for a conversation's timeline to be brought to its history: what a
/// reader of it does, so a conversation no one reads is not read whole for
/// nothing. Asked again while it waits or runs, it is one job.
pub(crate) fn wanted(node: &Arc<crate::Node>, resident: &ResidentId) {
    let job = Job { kind: KIND, key: resident.to_string(), unit: resident.to_string(), payload: Vec::new() };
    let (database, jobs) = (node.database.clone(), node.durable_jobs.clone());
    fastctx_platform::task::spawn(async move {
        let bytes = job.key.len() * 2 + 64;
        if database.write_relaxed(bytes, move |db| fastctx_jobs::submit(db, &job)).await.is_ok() {
            jobs.stir(KIND);
        }
    });
}

impl crate::Node {
    /// The content database, when it opened.
    pub(crate) fn content(&self) -> Result<&fastctx_content::Content, fastctx_types::ipc::Failure> {
        self.content
            .as_ref()
            .ok_or_else(|| fastctx_types::ipc::Failure::new("this machine's content database is unavailable"))
    }
}
