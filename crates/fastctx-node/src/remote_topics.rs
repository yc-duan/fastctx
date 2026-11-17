//! Waiting on another machine's topics for a page here (`Call::RemoteTopics`):
//! asked of it only when its facts say it tells them and the way to it
//! carries the asking, and that kept a while, so the pages of an older
//! machine, or of one reached through an older hub, go on reading on their
//! timers.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use fastctx_types::MachineName;
use fastctx_types::access::{Acting, HubId, Purpose, Written};
use fastctx_types::ipc::Failure;
use fastctx_types::remote::{RemoteCall, RemoteReply};
use fastctx_types::topics::{TopicsAsk, TopicsTold};

use crate::Node;

/// How long what a machine's facts said of its topics is taken as said.
const TOLD_FOR: Duration = Duration::from_secs(60);

/// A machine as reached through one hub: the same name on two hubs is two
/// machines.
type Reached = (HubId, MachineName);

/// Whether each machine tells its topics, and since when that is known.
static TELLS: LazyLock<Mutex<HashMap<Reached, (Instant, bool)>>> = LazyLock::new(Default::default);

/// Waits on `machine`'s topics as `request` asks; none when it does not tell
/// them, or the way to it does not carry the asking, for the page to read on
/// a timer.
pub(crate) async fn wait(
    node: &Arc<Node>,
    acting: &Acting,
    machine: Written,
    request: TopicsAsk,
) -> Result<Option<TopicsTold>, Failure> {
    request.validate().map_err(Failure::not_run)?;
    if node.is_here(&machine) {
        return Ok(Some(node.topics.told(&request, crate::topics::WAITS).await));
    }
    let mut refused = None;
    for (link, account) in node.speakers(acting, &machine, Purpose::Look).map_err(|error| Failure::new(error.to_string()))? {
        let hub = link.membership().id;
        if !tells(&link, &hub, &machine.name, &account).await? {
            return Ok(None);
        }
        let reply = match fastctx_net::cancellable::call(&link, &machine.name, &account, RemoteCall::Topics { request: request.clone() }).await {
            Ok(reply) => reply,
            // Not carried — a hub of a release before topics passes on no
            // asking it cannot read — the page reads on its timer a while.
            Err(_) => {
                TELLS.lock().expect("never poisoned").insert((hub, machine.name.clone()), (Instant::now(), false));
                return Ok(None);
            }
        };
        match reply {
            Ok(RemoteReply::Topics(told)) => return Ok(Some(told)),
            Err(error) if error.not_run => refused = Some(error),
            Err(error) => return Err(error),
            _ => return Err(Failure::new("the machine answered something other than its topics")),
        }
    }
    Err(refused.unwrap_or_else(|| Failure::not_run(format!("{} is not reachable", machine.name))))
}

/// Whether `machine` reached through `hub` tells its topics, as its facts
/// said within `TOLD_FOR`.
async fn tells(link: &fastctx_net::link::Link, hub: &HubId, machine: &MachineName, account: &fastctx_types::AccountName) -> Result<bool, Failure> {
    let key = (hub.clone(), machine.clone());
    if let Some((at, tells)) = TELLS.lock().expect("never poisoned").get(&key)
        && at.elapsed() < TOLD_FOR
    {
        return Ok(*tells);
    }
    let tells = match link.call(machine, account, Purpose::Look, RemoteCall::Facts).await {
        Ok(Ok(RemoteReply::Facts(facts))) => facts.topics,
        Ok(Ok(_)) => false,
        Ok(Err(error)) if error.not_run => false,
        Ok(Err(error)) => return Err(error),
        Err(error) => return Err(Failure::new(error.to_string())),
    };
    TELLS.lock().expect("never poisoned").insert(key, (Instant::now(), tells));
    Ok(tells)
}
