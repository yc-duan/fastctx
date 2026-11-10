//! The read-only looks a node takes for the World's own agents
//! (`world-senses.md`, 哨兵是世界自己的 agent): read a file, list a
//! directory, see the processes, the GPUs, memory and disks, and the
//! account's jobs and their output. Each is taken in the node's own code by
//! the machine looked at, for an account that may work on it, as `run`
//! there would be; none runs a program but `nvidia-smi`, which the node
//! runs itself.

mod files;
mod guard;
mod jobs;
mod machine;
mod redact;

use std::sync::Arc;

use fastctx_types::access::{Acting, HubAccount, Purpose, Written};
use fastctx_types::inspect::{Inspect, Inspected, MOST_BYTES};
use fastctx_types::ipc::Failure;
use fastctx_types::remote::{RemoteCall, RemoteReply};
use fastctx_types::{Address, MachineName};

use crate::Node;

/// Takes `inspect` on `machine` for the account `scope`: here, or asked of
/// that machine's node. With `evidence_of`, the folder where the World keeps
/// that conversation's evidence may be read too.
pub(crate) async fn look(
    node: &Arc<Node>,
    scope: &HubAccount,
    machine: &MachineName,
    inspect: Inspect,
    evidence_of: Option<Address>,
) -> Result<Inspected, Failure> {
    inspect.check().map_err(Failure::not_run)?;
    let link = node.link_of(&scope.hub).ok_or_else(|| Failure::not_run("the account's hub is not connected"))?;
    let acting = Acting::one(scope.hub.clone(), scope.account.clone());
    let written = Written { name: machine.clone(), hub: Some(link.hub_name()) };
    node.settled(&written).await;
    node.authorize_machine(&acting, &written)?;
    if node.is_here(&written) {
        return here(node, scope, inspect, evidence_of.as_ref()).await;
    }
    let asked = |error: fastctx_net::link::CallFailure| Failure::new(error.to_string());
    match node.call(&acting, &written, Purpose::Look, RemoteCall::Facts).await.map_err(asked)?? {
        RemoteReply::Facts(facts) if facts.inspects => {}
        _ => {
            return Err(Failure::not_run(format!(
                "{machine} runs a Fastctx too old to be looked at this way; update it"
            )));
        }
    }
    let call = RemoteCall::Inspect { inspect, evidence_of };
    match node.call(&acting, &written, Purpose::Hands, call).await.map_err(asked)?? {
        RemoteReply::Inspected(inspected) => Ok(inspected),
        _ => Err(Failure::new(format!("{machine} answered something other than what it saw"))),
    }
}

/// Takes `inspect` on this machine for the account `scope`.
pub(crate) async fn here(
    node: &Arc<Node>,
    scope: &HubAccount,
    inspect: Inspect,
    evidence_of: Option<&Address>,
) -> Result<Inspected, Failure> {
    inspect.check().map_err(Failure::not_run)?;
    node.authorize_here(&Acting::one(scope.hub.clone(), scope.account.clone()))?;
    let seen = match inspect {
        Inspect::Resources => machine::resources().await,
        Inspect::Processes { matching } => {
            fastctx_platform::task::blocking(move || machine::processes(matching.as_deref())).await?
        }
        Inspect::ReadFile { path, tail, matching } => {
            let guard = guard_for(node, scope, evidence_of)?;
            fastctx_platform::task::blocking(move || files::read(&guard, &path, tail, matching.as_deref())).await?
        }
        Inspect::ListDirectory { path, pattern } => {
            let guard = guard_for(node, scope, evidence_of)?;
            fastctx_platform::task::blocking(move || files::list(&guard, &path, pattern.as_deref())).await?
        }
        Inspect::Jobs => {
            let (node, scope) = (node.clone(), scope.clone());
            fastctx_platform::task::blocking(move || jobs::list(&node, &scope)).await?
        }
        Inspect::JobOutput { job, tail, matching } => {
            let (node, scope) = (node.clone(), scope.clone());
            fastctx_platform::task::blocking(move || jobs::output(&node, &scope, &job, tail, matching.as_deref()))
                .await?
        }
    };
    Ok(bounded(seen))
}

/// A look that found nothing to show, and says why.
fn noted(note: String) -> Inspected {
    Inspected { text: String::new(), note: Some(note) }
}

/// What a look may not read here, and the evidence folder it may.
fn guard_for(node: &Node, scope: &HubAccount, evidence_of: Option<&Address>) -> Result<guard::Guard, Failure> {
    let evidence = match evidence_of {
        Some(owner) => crate::placement::artifacts::read_directory(node, scope, owner)?,
        None => None,
    };
    Ok(guard::Guard::new(&node.root, evidence))
}

/// `seen` within what one look gives back, the beginning of its text kept.
fn bounded(mut seen: Inspected) -> Inspected {
    if seen.text.len() > MOST_BYTES {
        let mut end = MOST_BYTES;
        while !seen.text.is_char_boundary(end) {
            end -= 1;
        }
        let whole = seen.text.len();
        seen.text.truncate(end);
        let cut = format!("its first {end} of {whole} bytes are shown");
        seen.note = Some(match seen.note {
            Some(note) => format!("{note}; {cut}"),
            None => cut,
        });
    }
    seen
}
