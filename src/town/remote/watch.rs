//! Watching a resident across machines (`specs/world-network.md`, "订阅"). Watching a
//! resident who lives elsewhere subscribes to its machine and relays what comes; another
//! machine watching a resident who lives here is served the same history and events a watch
//! on this machine sees. Nothing is stored or queued: a watch that ends is gone, and the next
//! starts again from the history.

use crate::town::core::Town;
use crate::town::talk::{HistoryKeys, TalkEvent};
use crate::world::envelope::MAX_CONTROL_MESSAGE_BYTES;
use crate::world::subscription::Notice;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// The largest event one control message carries: its body travels as base64 (four bytes for
/// every three), beside a header, a signature and the frame, which stay well under 4 KiB, and
/// inside the subscription's wrapper (its id and field names, under 128 bytes).
const EVENT_BYTES: usize = (MAX_CONTROL_MESSAGE_BYTES - 4 * 1024) / 4 * 3 - 128;

/// What a subscriber asks another machine for: one resident's history and events.
#[derive(Debug, Deserialize, Serialize)]
struct Topic {
    watch: String,
}

/// The watches that cross machines, both ways.
#[derive(Default)]
pub(crate) struct Watches {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    /// Subscriptions this machine opened: where each one's events go. A relay is in place
    /// before its subscription is asked for, so nothing that comes under it finds no one.
    relays: HashMap<String, mpsc::UnboundedSender<TalkEvent>>,
    /// Subscriptions other machines opened here, and how to stop serving each.
    serving: HashMap<String, CancellationToken>,
}

/// A watch of a resident on another machine. Dropping it ends the subscription.
pub(crate) struct RemoteWatch {
    town: Arc<Town>,
    subscription: String,
    pub(crate) events: mpsc::UnboundedReceiver<TalkEvent>,
}

impl Drop for RemoteWatch {
    fn drop(&mut self) {
        self.town
            .watches
            .inner
            .lock()
            .relays
            .remove(&self.subscription);
        // The subscription may already be closed by the other side; then there is nothing to end.
        let _ = self.town.world().unsubscribe(&self.subscription);
    }
}

impl Town {
    /// Watches a resident who lives on `machine`: its history, then its events, as a watch
    /// there would give them.
    pub(crate) async fn watch_elsewhere(
        self: &Arc<Self>,
        machine: &str,
        id: &str,
    ) -> Result<RemoteWatch, String> {
        let subscription = crate::world::client::WorldClient::subscription_id()?;
        let (sender, events) = mpsc::unbounded_channel();
        self.watches
            .inner
            .lock()
            .relays
            .insert(subscription.clone(), sender);
        if let Err(error) = self
            .world()
            .subscribe(
                machine,
                &subscription,
                json!(Topic {
                    watch: id.to_string()
                }),
            )
            .await
        {
            self.watches.inner.lock().relays.remove(&subscription);
            return Err(error);
        }
        Ok(RemoteWatch {
            town: Arc::clone(self),
            subscription,
            events,
        })
    }

    /// Follows this machine's subscriptions for as long as the node runs: other machines
    /// starting to watch a resident here, events of residents watched from here, and the end
    /// of either.
    pub(crate) async fn follow_subscriptions(self: Arc<Self>) {
        loop {
            match self.world().subscriptions.next().await {
                Notice::Started { id, from, topic } => self.serve(id, from, topic),
                Notice::Event { id, value, .. } => {
                    let event =
                        serde_json::from_value::<TalkEvent>(value).unwrap_or_else(|error| {
                            TalkEvent::Gone {
                                reason: format!("an unreadable event arrived: {error}"),
                            }
                        });
                    self.relay(&id, event);
                }
                Notice::Closed { id, reason, .. } => {
                    let serving = self.watches.inner.lock().serving.remove(&id);
                    match serving {
                        Some(stop) => stop.cancel(),
                        None => self.relay_end(&id, reason),
                    }
                }
            }
        }
    }

    /// An event of a subscription this machine opened goes to its watcher; one whose watcher
    /// already went away has nowhere to go.
    fn relay(&self, subscription: &str, event: TalkEvent) {
        let mut inner = self.watches.inner.lock();
        if let Some(relay) = inner.relays.get(subscription)
            && relay.send(event).is_err()
        {
            inner.relays.remove(subscription);
        }
    }

    /// A subscription this machine opened has ended: its watcher hears why.
    fn relay_end(&self, subscription: &str, reason: String) {
        if let Some(relay) = self.watches.inner.lock().relays.remove(subscription) {
            let _ = relay.send(TalkEvent::Gone { reason });
        }
    }

    /// Serves another machine's watch of a resident who lives here, if that machine may talk
    /// to residents here (the `talk` grant).
    fn serve(self: &Arc<Self>, subscription: String, from: String, topic: Value) {
        let world = self.world();
        let allowed = world
            .grants
            .read()
            .allows(&from, "talk", self.machine(), &world.own_tags());
        let watched = serde_json::from_value::<Topic>(topic).map(|topic| topic.watch);
        let resident = match (allowed, watched) {
            (true, Ok(resident)) => resident,
            (false, _) => {
                return self.refuse(
                    &subscription,
                    format!("{} does not allow talk for \"{from}\".", self.machine()),
                );
            }
            (_, Err(error)) => {
                return self.refuse(&subscription, format!("unreadable watch request: {error}"));
            }
        };
        let stop = CancellationToken::new();
        self.watches
            .inner
            .lock()
            .serving
            .insert(subscription.clone(), stop.clone());
        let town = Arc::clone(self);
        tokio::spawn(async move {
            town.serve_watch(&subscription, &resident, stop).await;
            town.watches.inner.lock().serving.remove(&subscription);
        });
    }

    async fn serve_watch(&self, subscription: &str, resident: &str, stop: CancellationToken) {
        // The watcher can leave while the history is still being read.
        let watched = tokio::select! {
            () = stop.cancelled() => return,
            watched = self.watch(resident) => watched,
        };
        let (history, mut events) = match watched {
            Ok(watch) => watch,
            Err(reason) => return self.refuse(subscription, reason),
        };
        for event in history.iter().chain([&TalkEvent::Live]) {
            if !self.publish_to(subscription, resident, event) {
                return;
            }
        }
        let seen = HistoryKeys::of(&history);
        loop {
            let event = tokio::select! {
                () = stop.cancelled() => return,
                event = events.recv() => event,
            };
            let Some(event) = event else {
                self.refuse(
                    subscription,
                    format!("{resident}'s events ended on {}.", self.machine()),
                );
                return;
            };
            if !seen.repeats(&event) && !self.publish_to(subscription, resident, &event) {
                return;
            }
        }
    }

    /// Sends one event to a watcher on another machine; false once that watch is over.
    fn publish_to(&self, subscription: &str, resident: &str, event: &TalkEvent) -> bool {
        let event = self.fitted(resident, event);
        match self.world().publish_event(subscription, json!(event)) {
            Ok(()) => true,
            Err(error) => {
                self.log(format!("stopped serving a watch of {resident}: {error}"));
                false
            }
        }
    }

    /// The event itself, or, when its text is more than one control message carries, the same
    /// event with a note saying so in place of the text.
    fn fitted(&self, resident: &str, event: &TalkEvent) -> TalkEvent {
        let size = serde_json::to_vec(event)
            .map(|bytes| bytes.len())
            .unwrap_or(usize::MAX);
        if size <= EVENT_BYTES {
            return event.clone();
        }
        let note = |text: &str| {
            format!(
                "[{} characters, more than one World message carries; the full text is in {resident}'s session on {}]",
                text.chars().count(),
                self.machine()
            )
        };
        match event {
            TalkEvent::Words { from, text, item } => TalkEvent::Words {
                from: from.clone(),
                text: note(text),
                item: item.clone(),
            },
            TalkEvent::Message { item, text } => TalkEvent::Message {
                item: item.clone(),
                text: note(text),
            },
            TalkEvent::Delta { item, text } => TalkEvent::Delta {
                item: item.clone(),
                text: note(text),
            },
            TalkEvent::Tool {
                item,
                name,
                summary,
                done,
                failed,
            } => TalkEvent::Tool {
                item: item.clone(),
                name: name.clone(),
                summary: note(summary),
                done: *done,
                failed: *failed,
            },
            // The rest carry no free text and are a few bytes long.
            other => other.clone(),
        }
    }

    /// Ends a watch served here, telling the watcher why.
    fn refuse(&self, subscription: &str, reason: String) {
        let _ = self
            .world()
            .publish_event(subscription, json!(TalkEvent::Gone { reason }));
        let _ = self.world().unsubscribe(subscription);
    }
}
