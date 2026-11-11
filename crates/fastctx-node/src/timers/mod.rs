//! Persistent deadlines. The scheduler only indexes records; actions own their effects.

mod claims;
mod headers;
pub(crate) use headers::ensure as ensure_headers;
mod page;
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
    node.timers.get().ok_or_else(|| {
        Failure::not_run("the timer table is still recovering").capacity(fastctx_types::ipc::CapacityReason::Pending)
    })
}

pub(crate) async fn call(node: &Arc<Node>, call: TimerCall) -> Result<Reply, Failure> {
    call.validate().map_err(Failure::not_run)?;
    let store = table(node)?;
    if let TimerCall::List { owner, after, paged } = call {
        let page = store.page(owner.as_ref(), after.as_deref()).await.map_err(failure)?;
        return page_reply(page, paged);
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
        TimerCall::Cancel { id, handle } => {
            let value = handle.as_ref().map_or(id.as_str(), |handle| handle.as_str());
            match store.header(value, handle.is_some()).await.map_err(failure)? {
                Some(header) => match header.routing {
                    headers::Routing::Tell(resident) => resident,
                    headers::Routing::Sentinel => {
                        return Err(Failure::refused("sentinel timers are managed by their sentinel"));
                    }
                    headers::Routing::Goal => {
                        return Err(Failure::refused("goal timers are managed by the goal runtime"));
                    }
                },
                None => return Ok(Reply::Done),
            }
        }
        TimerCall::List { .. } => unreachable!(),
    };
    let gate = crate::control_queue::gate(node, &resident);
    let _action = gate.lock().await;
    let _place = node.places.using(resident.as_str()).await;
    match call {
        TimerCall::Set { id, owner, due, action, handle } => {
            node.residents.active(&resident)?;
            let value = handle.as_ref().map_or(id.as_str(), |handle| handle.as_str());
            let previous = store.header(value, handle.is_some()).await.map_err(failure)?;
            if previous.as_ref().is_some_and(|header| header.owner.as_ref() != Some(&owner)) {
                return Err(Failure::new("cancel a timer before changing its owner"));
            }
            let delivery = crate::speech::new_id("timer");
            if handle.is_some() || id.len() > 4096 {
                let previous = previous.ok_or_else(|| {
                    Failure::not_run(if handle.is_some() {
                        "no timer has this handle"
                    } else {
                        "a new timer identity exceeds 4096 bytes"
                    })
                })?;
                let timer = store.replace_header(previous, owner, due, action, delivery).await.map_err(failure)?;
                return Ok(timer.map_or(Reply::Done, |timer| Reply::Timers {
                    timers: vec![timer],
                    next: None,
                    oversized: Vec::new(),
                }));
            }
            let timer = store.set(id, owner, due, action, delivery).await.map_err(failure)?;
            Ok(Reply::Timers { timers: vec![timer], next: None, oversized: Vec::new() })
        }
        TimerCall::Cancel { id, handle } => {
            let value = handle.as_ref().map_or(id.as_str(), |handle| handle.as_str());
            if let Some(header) = store.header(value, handle.is_some()).await.map_err(failure)? {
                match &header.routing {
                    headers::Routing::Tell(current) if current == &resident => {}
                    headers::Routing::Tell(_) => {
                        return Err(Failure::not_run("the timer's conversation changed; try again"));
                    }
                    headers::Routing::Sentinel => {
                        return Err(Failure::refused("sentinel timers are managed by their sentinel"));
                    }
                    headers::Routing::Goal => {
                        return Err(Failure::refused("goal timers are managed by the goal runtime"));
                    }
                }
                store.cancel_header(header).await.map_err(failure)?;
            }
            Ok(Reply::Done)
        }
        TimerCall::List { .. } => unreachable!(),
    }
}

fn page_reply(mut page: page::Page, paged: bool) -> Result<Reply, Failure> {
    if !paged {
        if page.next.is_some() || !page.oversized.is_empty() {
            return Err(Failure::not_run(
                "the timer list exceeds 128 inspected rows, the 1 MiB reply limit or a stored row limit; use a paging-aware client (fastctx timer list --page)",
            ));
        }
        page.timers.sort_by(|a, b| a.id.cmp(&b.id));
    }
    Ok(Reply::Timers { timers: page.timers, next: page.next, oversized: page.oversized })
}

pub(crate) fn start(node: &Arc<Node>) {
    let node = node.clone();
    fastctx_platform::task::spawn(async move {
        let mut again = crate::again::Again::default();
        let mut last_error = None;
        let store = loop {
            let path = node.world.path().join("timers");
            match fastctx_platform::task::spawn_blocking(move || Store::open(&path)).await {
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
                let timer = match store.claim(Timestamp::now()).await {
                    Ok(Some(timer)) => timer,
                    Ok(None) => break,
                    Err(error) => {
                        node.log.line(format_args!("claiming timer: {error}"));
                        break;
                    }
                };
                let node = node.clone();
                let store = store.clone();
                running.spawn(async move {
                    let _claim = Claimed(store.clone(), timer.clone());
                    let result = execute(&node, &store, &timer).await;
                    if let Err(error) = result {
                        if timer.error.as_deref() != Some(&error) {
                            node.log.line(format_args!("timer {}: {error}", timer.id));
                        }
                        if let Err(error) = store.retry(&timer, Timestamp::now(), error).await {
                            node.log.line(format_args!("keeping timer {} retry: {error}", timer.id));
                            store.release(&timer, Timestamp::now());
                        }
                    }
                });
            }
            let wait = if running.len() == CONCURRENT {
                HEARTBEAT
            } else {
                store
                    .next()
                    .await
                    .unwrap_or_else(|error| {
                        node.log.line(format_args!("reading timer deadline: {error}"));
                        None
                    })
                    .map_or(HEARTBEAT, |at| {
                        Duration::from_secs_f64(at.duration_since(Timestamp::now()).as_secs_f64().max(0.0))
                            .min(HEARTBEAT)
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

async fn execute(node: &Arc<Node>, store: &Arc<Store>, timer: &Timer) -> Result<(), String> {
    match &timer.action {
        Action::Sentinel { .. } => crate::sentinels::timers::fire(node, store, timer).await,
        Action::Goal { .. } => crate::goals::engine::fire(node, store, timer).await.map_err(|error| error.message),
        Action::Tell { resident, .. } => {
            let gate = crate::control_queue::gate(node, resident);
            let _action = gate.lock().await;
            let _place = node.places.using(resident.as_str()).await;
            if node.residents.deleted(resident).is_some() || node.residents.removed(resident).is_some() {
                store.complete(timer).await.map_err(|error| error.to_string())?;
                return Ok(());
            }
            let Some(timer) = store.begin(timer, Timestamp::now()).await.map_err(|error| error.to_string())? else {
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
                    told: None,
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
            store.complete(&timer).await.map_err(|error| error.to_string())?;
            Ok(())
        }
    }
}

fn failure(error: io::Error) -> Failure {
    crate::record_migration::failure(error)
}

struct Claimed(Arc<Store>, Timer);
impl Drop for Claimed {
    fn drop(&mut self) {
        self.0.unclaim(&self.1);
    }
}
