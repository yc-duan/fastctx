//! Concurrent questions owned by the request asking them, shared by nodes
//! and devices. Each hub is refreshed once per round.

use std::any::Any;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;

use fastctx_types::remote::{RemoteCall, RemoteReply};
use fastctx_types::{AccountName, MachineName};
use futures_util::future::{BoxFuture, FutureExt, Shared, join_all, ready};
use futures_util::stream::{FuturesUnordered, StreamExt};
use jiff::Timestamp;

use crate::link::{CallFailure, Link};

/// How one machine answered.
pub enum Heard {
    Answered(RemoteReply),
    Offline(Option<Timestamp>),
    NoAnswer(String),
}

/// A machine to ask, through one hub and for one account.
pub struct Reachable {
    pub link: Arc<Link>,
    pub name: MachineName,
    pub account: AccountName,
}

/// Asks every machine concurrently, refreshing each distinct link once.
/// Dropping this future drops every directory wait and machine call. Each
/// call retains its own contract, including connection waits, and answers
/// under its target's name, even if polling it panics. Directory refreshes
/// include lock and connection waits in their deadline. Results are sorted
/// by machine name.
pub async fn ask_each(
    machines: Vec<Reachable>,
    call: impl Fn(&MachineName) -> RemoteCall,
) -> Vec<(MachineName, Heard)> {
    type Refresh = Shared<BoxFuture<'static, Result<(), String>>>;
    let mut refreshes: Vec<(Arc<Link>, Refresh)> = Vec::new();
    ask(machines, call, |link| match refreshes.iter().find(|(asked, _)| Arc::ptr_eq(asked, link)) {
        Some((_, fresh)) => fresh.clone(),
        None => {
            let asked = link.clone();
            let fresh = async move { refresh(&asked).await }.boxed().shared();
            refreshes.push((link.clone(), fresh.clone()));
            fresh
        }
    })
    .await
}

/// Asks hubs concurrently, passing each one's fresh directory result to
/// `ask`. No task outlives the future returned here; hub order is retained.
pub async fn on_hubs<T, F: Future<Output = T>>(
    links: Vec<Arc<Link>>,
    ask: impl Fn(Arc<Link>, Result<(), String>) -> F,
) -> Vec<T> {
    let ask = &ask;
    join_all(links.into_iter().map(|link| async move {
        let fresh = refresh(&link).await;
        ask(link, fresh).await
    }))
    .await
}

/// Asks machines whose hub was already refreshed by this round. A failed
/// refresh is reported for every target without using the cached presence.
pub async fn ask_refreshed(
    machines: Vec<Reachable>,
    call: impl Fn(&MachineName) -> RemoteCall,
    fresh: Result<(), String>,
) -> Vec<(MachineName, Heard)> {
    ask(machines, call, |_| ready(fresh.clone())).await
}

async fn refresh(link: &Link) -> Result<(), String> {
    let within = crate::link::within(&crate::wire::Question::Directory);
    let work = async {
        fastctx_platform::doing::outside("the hub's directory", within, link.refresh())
            .await
            .map_err(|late| late.to_string())?
    };
    AssertUnwindSafe(work).catch_unwind().await.unwrap_or_else(|panic| Err(panicked(panic)))
}

async fn ask<F: Future<Output = Result<(), String>>>(
    machines: Vec<Reachable>,
    call: impl Fn(&MachineName) -> RemoteCall,
    mut refreshed: impl FnMut(&Arc<Link>) -> F,
) -> Vec<(MachineName, Heard)> {
    let mut asks = FuturesUnordered::new();
    for Reachable { link, name, account } in machines {
        let fresh = refreshed(&link);
        let call = call(&name);
        asks.push(async move {
            let answer = AssertUnwindSafe(async {
                if let Err(error) = fresh.await {
                    return Heard::NoAnswer(format!(
                        "whether it is connected is not known: the hub could not be asked ({error})"
                    ));
                }
                let seen = link.view().member(&name).cloned();
                match seen {
                    Some(seen) if seen.online => {
                        let purpose = call.purpose();
                        let within = call.contract().within;
                        let work = link.call(&name, &account, purpose, call);
                        let answer = match within {
                            Some(within) => {
                                match fastctx_platform::doing::outside(format!("{name}'s answer"), within, work).await {
                                    Ok(answer) => answer,
                                    Err(late) => return Heard::NoAnswer(late.to_string()),
                                }
                            }
                            None => work.await,
                        };
                        match answer {
                            Ok(Ok(reply)) => Heard::Answered(reply),
                            Ok(Err(failure)) => Heard::NoAnswer(failure.message),
                            Err(CallFailure::Offline { last_seen, .. }) => Heard::Offline(last_seen),
                            Err(failure) => Heard::NoAnswer(failure.to_string()),
                        }
                    }
                    Some(seen) => Heard::Offline(seen.since),
                    None => Heard::NoAnswer("the hub did not say whether it is connected".to_string()),
                }
            })
            .catch_unwind()
            .await
            .unwrap_or_else(|panic| Heard::NoAnswer(panicked(panic)));
            (name, answer)
        });
    }
    let mut heard = Vec::new();
    while let Some(answer) = asks.next().await {
        heard.push(answer);
    }
    heard.sort_by(|a, b| a.0.cmp(&b.0));
    heard
}

fn panicked(panic: Box<dyn Any + Send>) -> String {
    let reason = panic.downcast_ref::<String>().map(String::as_str).or_else(|| panic.downcast_ref::<&str>().copied());
    match reason {
        Some(reason) => format!("asking around panicked: {reason}"),
        None => "asking around panicked".to_string(),
    }
}
