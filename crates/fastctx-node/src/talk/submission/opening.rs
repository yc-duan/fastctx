//! Creating a conversation has no repeatable host-side effect.

use std::sync::Arc;
use std::time::Duration;

use fastctx_types::access::{Acting, Written};
use fastctx_types::agents::{Choices, SessionOptions};
use fastctx_types::attachments::Attachment;
use fastctx_types::ipc::Failure;
use fastctx_types::residents::Landed;
use fastctx_types::{Address, AgentName, MachineName, ResidentId, TownName};
use serde::{Deserialize, Serialize};

use crate::Node;

const FAMILY: crate::submitted::Family = crate::submitted::Family {
    name: "call/open",
    at_once: 8,
    within: Duration::from_secs(300),
    stays: Duration::from_secs(240),
    // A second thread or background session is a second conversation.
    again: false,
};

#[derive(Serialize, Deserialize)]
struct Opening {
    agent: AgentName,
    town: Option<TownName>,
    text: String,
    options: SessionOptions,
    // Upload receipts only; file and image bytes stay in the attachment store.
    attachments: Vec<Attachment>,
    choices: Choices,
}

#[derive(Serialize, Deserialize)]
enum Asked {
    Route { target: Option<Written>, opening: Opening },
    Here { typed_on: MachineName, acting: Acting, opening: Opening },
}

pub(super) fn register(node: &Arc<Node>) {
    crate::submitted::register(node, FAMILY, |node: Arc<Node>, asked: Asked| async move {
        match asked {
            Asked::Route { target, opening } => {
                let Opening { agent, town, text, options, attachments, choices } = opening;
                crate::talk::open(&node, agent, town, target, text, options, attachments, choices).await
            }
            Asked::Here { typed_on, acting, opening } => {
                let Opening { agent, town, text, options, attachments, choices } = opening;
                crate::talk::open_here(&node, &typed_on, acting, agent, town, text, options, attachments, choices)
                    .await
                    .map(|(resident, landed)| (Address { machine: node.machine(), resident }, landed))
            }
        }
    });
}

fn supported(agent: &AgentName) -> Result<(), Failure> {
    if matches!(agent.as_str(), "codex" | "claude") {
        Ok(())
    } else {
        Err(Failure::not_run(format!("the World opens codex and claude sessions; it cannot open {agent} ones")))
    }
}

/// The upload's conversation already has an identity. Otherwise the operation's
/// stable name is its unit, while the original worker allocates the resident ID.
fn unit(attachments: &[Attachment]) -> Result<String, Failure> {
    if let Some(first) = attachments.first() {
        return Ok(first.conversation().resident.to_string());
    }
    let name = match fastctx_net::submission::current() {
        Some(submission) => submission.name,
        // An unnamed legacy call remains one new operation for each asking.
        None => fastctx_net::sign::random::<16>()
            .map_err(|error| Failure::not_run(format!("naming the opening: {error}")))?
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    };
    Ok(format!("open/{}", fastctx_hands::files::content_hash(name.as_bytes())))
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn open(
    node: &Arc<Node>,
    agent: AgentName,
    town: Option<TownName>,
    target: Option<Written>,
    text: String,
    options: SessionOptions,
    attachments: Vec<Attachment>,
    choices: Choices,
) -> Result<(Address, Landed), Failure> {
    supported(&agent)?;
    if let Some(account) = &choices.account {
        if !node.serves(&Acting::Here, &account.hub, &account.account) {
            return Err(Failure::not_run(format!("this computer is not logged in to the account {}", account.account)));
        }
    } else {
        let count = node.links().iter().map(|link| link.view().me().map_or(0, |me| me.logins.len())).sum::<usize>();
        if count > 1 {
            return Err(Failure::not_run(
                "this computer is logged in to more than one account; choose the one the conversation works for",
            ));
        }
    }
    let unit = unit(&attachments)?;
    let opening = Opening { agent, town, text, options, attachments, choices };
    crate::submitted::submit(node, &FAMILY, &unit, &Asked::Route { target, opening }).await
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn open_here(
    node: &Arc<Node>,
    typed_on: &MachineName,
    acting: Acting,
    agent: AgentName,
    town: Option<TownName>,
    text: String,
    options: SessionOptions,
    attachments: Vec<Attachment>,
    choices: Choices,
) -> Result<(ResidentId, Landed), Failure> {
    supported(&agent)?;
    crate::logins::workbench::may_start(node, &acting, agent.as_str(), choices.home.as_deref())?;
    if let Some(first) = attachments.first()
        && (!node.is_me(&first.conversation().machine) || first.town != town)
    {
        return Err(Failure::not_run("the upload draft belongs to another machine or town"));
    }
    let unit = unit(&attachments)?;
    let opening = Opening { agent, town, text, options, attachments, choices };
    let (address, landed): (Address, Landed) =
        crate::submitted::submit(node, &FAMILY, &unit, &Asked::Here { typed_on: typed_on.clone(), acting, opening })
            .await?;
    Ok((address.resident, landed))
}
