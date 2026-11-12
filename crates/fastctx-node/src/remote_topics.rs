//! Waiting on another machine's topics for a page here (`Call::RemoteTopics`):
//! asked of it only when its facts say it tells them, and that kept a while,
//! so the pages of an older machine go on reading on their timers.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use fastctx_types::access::{Acting, Purpose, Written};
use fastctx_types::ipc::Failure;
use fastctx_types::remote::{RemoteCall, RemoteReply};
use fastctx_types::topics::{TopicsAsk, TopicsTold};
use fastctx_types::MachineName;

use crate::Node;

/// How long what a machine's facts said of its topics is taken as said.
const TOLD_FOR: Duration = Duration::from_secs(60);

static TELLS: LazyLock<Mutex<HashMap<MachineName, (Instant, bool)>>> = LazyLock::new(Default::default);

/// Waits on `machine`'s topics as `request` asks; none when it does not tell
/// them, for the page to read on a timer.
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
        if !tells(&link, &machine.name, &account).await? {
            return Ok(None);
        }
        let reply = fastctx_net::cancellable::call(&link, &machine.name, &account, RemoteCall::Topics {
            request: request.clone(),
        })
        .await
        .map_err(|error| Failure::new(error.to_string()))?;
        match reply {
            Ok(RemoteReply::Topics(told)) => return Ok(Some(told)),
            Err(error) if error.not_run => refused = Some(error),
            Err(error) => return Err(error),
            _ => return Err(Failure::new("the machine answered something other than its topics")),
        }
    }
    Err(refused.unwrap_or_else(|| Failure::not_run(format!("{} is not reachable", machine.name))))
}

/// Whether `machine` tells its topics, as its facts said within `TOLD_FOR`.
async fn tells(link: &fastctx_net::link::Link, machine: &MachineName, account: &fastctx_types::AccountName) -> Result<bool, Failure> {
    if let Some((at, tells)) = TELLS.lock().expect("never poisoned").get(machine)
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
    TELLS.lock().expect("never poisoned").insert(machine.clone(), (Instant::now(), tells));
    Ok(tells)
}
