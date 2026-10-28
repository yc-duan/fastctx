//! Concurrent questions owned by the request asking them, shared by nodes
//! and devices. Each hub is refreshed once per round.

pub mod answers;

use std::any::Any;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;

use fastctx_types::access::Written;
use fastctx_types::observation::MachineSource;
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

/// A machine to ask, through one hub and for one account. `written` is
/// how the asker writes it: with its hub where another hub the asker is on
/// has a machine of that name, so the answer can be told apart and sent
/// back through the hub it came by (`world-network.md`, 机器的名字).
pub struct Reachable {
    pub link: Arc<Link>,
    pub name: MachineName,
    pub account: AccountName,
    pub key: crate::sign::PublicKey,
    pub written: Written,
}

/// How a machine named `name` on `link`'s hub is written among `links`: its
/// name, with the hub where another of them has a machine of that name,
/// the same computer or another.
pub fn written_among(links: &[Arc<Link>], link: &Link, name: &MachineName) -> Written {
    let elsewhere =
        links.iter().any(|other| other.membership().id != link.membership().id && other.view().member(name).is_some());
    Written { name: name.clone(), hub: elsewhere.then(|| link.hub_name()) }
}

/// The target and account of a completed question, retained even on failure.
#[derive(Clone, Debug)]
pub struct Asked {
    pub source: MachineSource,
    pub account: AccountName,
}

impl Asked {
    pub fn name(&self) -> &MachineName {
        &self.source.written.name
    }
}

/// Asks every machine concurrently, refreshing each distinct link once.
/// Dropping this future drops every directory wait and machine call. Each
/// call retains its own contract, including connection waits, and answers
/// with its target and account, even if polling it panics. Directory refreshes
/// include lock and connection waits in their deadline. Results are sorted
/// by machine name.
pub async fn ask_each(machines: Vec<Reachable>, call: impl Fn(&MachineName) -> RemoteCall) -> Vec<(Asked, Heard)> {
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
) -> Vec<(Asked, Heard)> {
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

/// The most machines one round asks at once: a network of hundreds is asked
/// a few dozen at a time (`world-concurrency.md`). One offline is not asked.
const ASKING_AT_ONCE: usize = 32;

async fn ask<F: Future<Output = Result<(), String>>>(
    machines: Vec<Reachable>,
    call: impl Fn(&MachineName) -> RemoteCall,
    mut refreshed: impl FnMut(&Arc<Link>) -> F,
) -> Vec<(Asked, Heard)> {
    let mut asks = FuturesUnordered::new();
    let asking = Arc::new(tokio::sync::Semaphore::new(ASKING_AT_ONCE));
    let sources: Vec<_> = machines
        .iter()
        .map(|machine| MachineSource {
            hub: machine.link.membership().id.clone(),
            key: machine.key.to_string(),
            written: machine.written.clone(),
        })
        .collect();
    for (Reachable { link, name, account, key, .. }, mut source) in machines.into_iter().zip(sources) {
        let fresh = refreshed(&link);
        let call = call(&name);
        let asking = asking.clone();
        asks.push(async move {
            let answer = AssertUnwindSafe(async {
                if let Err(error) = fresh.await {
                    return Heard::NoAnswer(format!(
                        "whether it is connected is not known: the hub could not be asked ({error})"
                    ));
                }
                let seen = link.view().machines().find(|machine| machine.key == key).cloned();
                if let Some(seen) = &seen {
                    source.written.name = seen.name.clone();
                }
                match seen {
                    Some(seen) if seen.online => {
                        let _turn = asking.acquire().await;
                        let name = &seen.name;
                        let purpose = call.purpose();
                        let within = call.contract().within;
                        let work = link.call(name, &account, purpose, call);
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
            (Asked { source, account }, answer)
        });
    }
    let mut heard = Vec::new();
    while let Some(answer) = asks.next().await {
        heard.push(answer);
    }
    heard.sort_by(|a, b| a.0.source.written.cmp(&b.0.source.written));
    heard
}

fn panicked(panic: Box<dyn Any + Send>) -> String {
    let reason = panic.downcast_ref::<String>().map(String::as_str).or_else(|| panic.downcast_ref::<&str>().copied());
    match reason {
        Some(reason) => format!("asking around panicked: {reason}"),
        None => "asking around panicked".to_string(),
    }
}
