//! Async admission for call facts; filesystem import and SQLite waits stay off
//! the coordinating runtime, including legacy executors' large progress frames.

use super::*;
use fastctx_types::run_facts::JobFacts;
use fastctx_types::{MachineName, ResidentId};
use std::sync::Arc;

async fn work<T: Send + 'static>(
    node: &Arc<Node>,
    action: impl FnOnce(&Node) -> io::Result<T> + Send + 'static,
) -> io::Result<T> {
    let node = node.clone();
    tokio::task::spawn_blocking(move || action(&node)).await.map_err(io::Error::other)?
}

pub(crate) async fn begin_for(
    node: &Arc<Node>,
    resident: &ResidentId,
    id: &str,
    machine: MachineName,
    turn: Option<String>,
    script: &str,
) -> io::Result<()> {
    let (resident, id, script) = (resident.clone(), id.to_string(), script.to_string());
    work(node, move |node| storage::begin(node, &resident, &id, machine, turn, &script)).await
}

pub(crate) async fn failed_for(node: &Arc<Node>, resident: &ResidentId, id: &str, error: &str) -> io::Result<()> {
    let (resident, id, error) = (resident.clone(), id.to_string(), error.to_string());
    work(node, move |node| storage::failed(node, &resident, &id, &error)).await
}

pub(crate) async fn record_failure(node: &Arc<Node>, resident: &ResidentId, id: &str, error: &str) -> io::Result<()> {
    let (resident, id, error) = (resident.clone(), id.to_string(), error.to_string());
    work(node, move |node| storage::record_failure(node, &resident, &id, &error)).await
}

pub(crate) async fn legacy_reply(
    node: &Arc<Node>,
    resident: &ResidentId,
    id: &str,
    reply: &RunReply,
) -> io::Result<()> {
    let (resident, id, reply) = (resident.clone(), id.to_string(), reply.clone());
    work(node, move |node| storage::legacy_reply(node, &resident, &id, &reply)).await
}

pub(crate) async fn keep_for(node: &Arc<Node>, resident: &ResidentId, id: &str, facts: &RunFacts) -> io::Result<()> {
    keep(node, resident, id, facts, true).await
}

pub(crate) async fn keep_reply(node: &Arc<Node>, resident: &ResidentId, id: &str, facts: &RunFacts) -> io::Result<()> {
    keep(node, resident, id, facts, false).await
}

async fn keep(node: &Arc<Node>, resident: &ResidentId, id: &str, facts: &RunFacts, display: bool) -> io::Result<()> {
    let (resident, id, facts) = (resident.clone(), id.to_string(), facts.clone());
    work(node, move |node| {
        if !node.residents.knows(&resident) {
            return Err(io::Error::other("unknown resident"));
        }
        storage::keep(&node.world.conversation(&resident), &id, &facts, display)
    })
    .await
}

pub(crate) async fn receive(node: &Arc<Node>, resident: ResidentId, update: JobFacts) -> io::Result<()> {
    work(node, move |node| storage::receive(node, &resident, &update)).await
}
