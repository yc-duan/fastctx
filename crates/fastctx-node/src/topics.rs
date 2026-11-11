//! The topics a reader waits on (`fastctx_types::topics`): what changes a
//! piece of the node's state says so by the topic's name, and whoever reads
//! that state is told, where it would otherwise ask again on a timer.
//!
//! The counts live for one run of the node. A reader that kept counts of an
//! earlier run is told every topic it asked for, and reads them all again.

use fastctx_types::topics::{TopicAt, TopicsAsk, TopicsTold};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

/// How long one asking waits for a change before it is answered with none.
pub(crate) const WAITS: Duration = Duration::from_secs(25);

pub(crate) struct Topics {
    run: String,
    revisions: Mutex<HashMap<&'static str, u64>>,
    moved: tokio::sync::watch::Sender<u64>,
}

impl Topics {
    pub(crate) fn new() -> Self {
        let started =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |since| since.as_nanos());
        Self {
            run: format!("{started:x}-{:x}", std::process::id()),
            revisions: Mutex::new(HashMap::new()),
            moved: tokio::sync::watch::channel(0).0,
        }
    }

    /// Says that what `topic` names changed. Called after the change is
    /// where a reader finds it.
    pub(crate) fn changed(&self, topic: &'static str) {
        *self.revisions.lock().expect("never poisoned").entry(topic).or_default() += 1;
        self.moved.send_modify(|moves| *moves = moves.wrapping_add(1));
    }

    /// Where the topics of `known` stand: all of them, or only those past
    /// where the reader read them.
    fn standing(&self, known: &[TopicAt], all: bool) -> Vec<TopicAt> {
        let revisions = self.revisions.lock().expect("never poisoned");
        known
            .iter()
            .filter_map(|known| {
                let revision = revisions.get(known.topic.as_str()).copied().unwrap_or(0);
                (all || revision > known.revision).then(|| TopicAt { topic: known.topic.clone(), revision })
            })
            .collect()
    }

    /// Answers once a topic asked for is past where its reader read it, or
    /// with none after `within`.
    pub(crate) async fn told(&self, asked: &TopicsAsk, within: Duration) -> TopicsTold {
        let told = |changed| TopicsTold { run: self.run.clone(), changed };
        if asked.run.as_deref() != Some(self.run.as_str()) {
            return told(self.standing(&asked.known, true));
        }
        // Watched before it is looked at, so a change between the two is seen.
        let mut moved = self.moved.subscribe();
        let waiting = async {
            loop {
                let changed = self.standing(&asked.known, false);
                if !changed.is_empty() || moved.changed().await.is_err() {
                    return changed;
                }
            }
        };
        told(tokio::time::timeout(within, waiting).await.unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(topic: &str, revision: u64) -> TopicAt {
        TopicAt { topic: topic.into(), revision }
    }

    #[test]
    fn a_reader_is_told_what_moved_past_where_it_read() {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_time().build().expect("a runtime");
        runtime.block_on(async {
            let topics = Topics::new();
            topics.changed("logins");
            // A reader that was told no run is told every topic it asks for.
            let first =
                topics.told(&TopicsAsk { run: None, known: vec![at("logins", 0), at("other", 0)] }, WAITS).await;
            assert_eq!(first.changed, [at("logins", 1), at("other", 0)]);
            let known = TopicsAsk { run: Some(first.run.clone()), known: first.changed };
            // Nothing changed: it waits, and is answered with none.
            let none = topics.told(&known, Duration::from_millis(30)).await;
            assert!(none.changed.is_empty());
            // A change while it waits answers it with the topic that moved.
            let (waiting, _) = tokio::join!(topics.told(&known, WAITS), async {
                tokio::time::sleep(Duration::from_millis(10)).await;
                topics.changed("other");
            });
            assert_eq!(waiting.changed, [at("other", 1)]);
            // Counts of another run compare with nothing here.
            let stale = TopicsAsk { run: Some("an-earlier-run".into()), known: vec![at("logins", 9)] };
            assert_eq!(topics.told(&stale, WAITS).await.changed, [at("logins", 1)]);
        });
    }
}
