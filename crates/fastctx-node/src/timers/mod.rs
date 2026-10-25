//! Persistent deadlines. The scheduler only indexes records; actions own their effects.

pub mod store;
mod words;

use std::io;
use std::sync::Arc;
use std::time::Duration;

use fastctx_types::Reply;
use fastctx_types::ipc::Failure;
use fastctx_types::residents::{Delivery, Notice};
use fastctx_types::session::Speaker;
use fastctx_types::timers::{Action, Timer, TimerCall};
use jiff::Timestamp;

use crate::Node;
use store::Store;

const HEARTBEAT: Duration = Duration::from_secs(30);
const CONCURRENT: usize = 32;

pub(crate) fn table(node: &Node) -> Result<&Arc<Store>, Failure> {
    node.timers.get().ok_or_else(|| Failure::new("the timer table is still recovering"))
}

pub(crate) async fn call(node: &Arc<Node>, call: TimerCall) -> Result<Reply, Failure> {
    let store = table(node)?;
    if let TimerCall::List { owner } = call {
        let timers =
            store.list(owner.as_ref()).into_iter().filter(|timer| !matches!(timer.action, Action::Sentinel { .. }));
        return Ok(Reply::Timers { timers: timers.collect() });
    }
    let resident = match &call {
        TimerCall::Set { action: Action::Sentinel { .. }, .. } => {
            return Err(Failure::refused("sentinel timers are managed by their sentinel"));
        }
        TimerCall::Set { action: Action::Goal { .. }, .. } => {
            return Err(Failure::refused("goal timers are managed by the goal runtime"));
        }
        TimerCall::Set { owner, action: Action::Tell { resident, .. }, .. } => {
            if !matches!(owner, fastctx_types::timers::Owner::Conversation { .. })
                || owner.conversation() != Some(resident)
            {
                return Err(Failure::new("a conversation timer must tell its own conversation"));
            }
            resident.clone()
        }
        TimerCall::Cancel { id } => match store.get(id) {
            Some(timer) => match timer.action {
                Action::Tell { resident, .. } => resident,
                Action::Sentinel { .. } => {
                    return Err(Failure::refused("sentinel timers are managed by their sentinel"));
                }
                Action::Goal { .. } => return Err(Failure::refused("goal timers are managed by the goal runtime")),
            },
            None => return Ok(Reply::Done),
        },
        TimerCall::List { .. } => unreachable!(),
    };
    let gate = crate::control_queue::gate(node, &resident);
    let _action = gate.lock().await;
    let _place = node.places.using(resident.as_str()).await;
    match call {
        TimerCall::Set { id, owner, due, action } => {
            node.residents.active(&resident)?;
            if store.get(&id).is_some_and(|timer| timer.owner != owner) {
                return Err(Failure::new("cancel a timer before changing its owner"));
            }
            let timer = store.set(id, owner, due, action, crate::speech::new_id("timer")).map_err(failure)?;
            Ok(Reply::Timers { timers: vec![timer] })
        }
        TimerCall::Cancel { id } => {
            store.cancel(&id).map_err(failure)?;
            Ok(Reply::Done)
        }
        TimerCall::List { .. } => unreachable!(),
    }
}

pub(crate) fn start(node: &Arc<Node>) {
    let node = node.clone();
    tokio::spawn(async move {
        let mut again = crate::again::Again::default();
        let mut last_error = None;
        let store = loop {
            let path = node.world.path().join("timers");
            match tokio::task::spawn_blocking(move || Store::open(&path)).await {
                Ok(Ok(store)) => break Arc::new(store),
                result => {
                    let error = match result {
                        Ok(Err(error)) => error.to_string(),
                        Err(error) => error.to_string(),
                        _ => unreachable!(),
                    };
                    if last_error.as_ref() != Some(&error) {
                        node.log.line(format_args!("recovering timers: {error}"));
                        last_error = Some(error);
                    }
                    tokio::time::sleep(again.next()).await;
                }
            }
        };
        let _ = node.timers.set(store.clone());
        let mut running = tokio::task::JoinSet::new();
        loop {
            // notify_one retains a permit when a mutation happens before this is polled.
            let changed = store.changed.notified();
            while running.len() < CONCURRENT {
                let Some(timer) = store.claim(Timestamp::now()) else { break };
                let node = node.clone();
                let store = store.clone();
                running.spawn(async move {
                    let result = execute(&node, &store, &timer).await;
                    if let Err(error) = result {
                        if timer.error.as_deref() != Some(&error) {
                            node.log.line(format_args!("timer {}: {error}", timer.id));
                        }
                        if let Err(error) = store.retry(&timer, Timestamp::now(), error) {
                            node.log.line(format_args!("keeping timer {} retry: {error}", timer.id));
                            store.release(&timer, Timestamp::now());
                        }
                    }
                });
            }
            let wait = if running.len() == CONCURRENT {
                HEARTBEAT
            } else {
                store.next().map_or(HEARTBEAT, |at| {
                    Duration::from_secs_f64(at.duration_since(Timestamp::now()).as_secs_f64().max(0.0)).min(HEARTBEAT)
                })
            };
            tokio::select! {
                () = changed => {},
                () = tokio::time::sleep(wait) => {},
                result = running.join_next(), if !running.is_empty() => {
                    if let Some(Err(error)) = result {
                        node.log.line(format_args!("timer worker ended unexpectedly: {error}"));
                    }
                }
            }
        }
    });
}

async fn execute(node: &Arc<Node>, store: &Store, timer: &Timer) -> Result<(), String> {
    match &timer.action {
        Action::Sentinel { .. } => crate::sentinels::timers::fire(node, store, timer),
        Action::Goal { .. } => crate::goals::engine::fire(node, store, timer).await.map_err(|error| error.message),
        Action::Tell { resident, .. } => {
            let gate = crate::control_queue::gate(node, resident);
            let _action = gate.lock().await;
            let _place = node.places.using(resident.as_str()).await;
            if node.residents.deleted(resident).is_some() || node.residents.removed(resident).is_some() {
                store.complete(timer).map_err(|error| error.to_string())?;
                return Ok(());
            }
            let Some(timer) = store.begin(timer, Timestamp::now()).map_err(|error| error.to_string())? else {
                return Ok(());
            };
            node.residents.active(resident).map_err(|error| error.message)?;
            crate::control_queue::drain(node, resident).await.map_err(|error| error.message)?;
            let management = node.residents.management(resident).map_err(|error| error.message)?;
            let _reading = management.read().await;
            let Action::Tell { text, .. } = &timer.action else { unreachable!() };
            let delivery = Delivery {
                id: timer.delivery.clone(),
                at: timer.fired_at.expect("begin records the firing"),
                notice: Notice::Words {
                    from: Speaker::World,
                    text: words::firing(&timer, text),
                    goal_report: None,
                    goal_question: None,
                },
            };
            if let Err(error) =
                crate::speech::tell_here_locked(node, resident, delivery, crate::speech::Owed::Nobody).await
            {
                if !crate::lifecycle::refused::reported(&error) {
                    return Err(error.message);
                }
                node.log.line(format_args!("timer {} was not delivered: {error}", timer.id));
            }
            store.complete(&timer).map_err(|error| error.to_string())?;
            Ok(())
        }
    }
}

pub(crate) fn delete_conversation(node: &Node, resident: &fastctx_types::ResidentId) -> io::Result<()> {
    table(node).map_err(|error| io::Error::other(error.message))?.delete_conversation(resident)
}

fn failure(error: io::Error) -> Failure {
    Failure::new(format!("recording timers: {error}"))
}
