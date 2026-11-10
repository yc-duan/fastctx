//! Asking creates a colleague's thread; its eventual answer uses the inbox.

use std::sync::Arc;
use std::time::Duration;

use fastctx_types::access::Acting;
use fastctx_types::collab::{Caller, Started};
use fastctx_types::ipc::Failure;
use fastctx_types::{Address, ResidentId, TownName};
use serde::{Deserialize, Serialize};

use crate::Node;

const FAMILY: crate::submitted::Family = crate::submitted::Family {
    name: "call/ask",
    at_once: 8,
    within: Duration::from_secs(300),
    stays: Duration::from_secs(240),
    // A retry of creation can create another colleague.
    again: false,
};

#[derive(Serialize, Deserialize)]
enum Asked {
    Route {
        asker: ResidentId,
        label: String,
        prompt: String,
        callee: Option<String>,
        login: Option<Caller>,
    },
    Here {
        requester: Address,
        turn: Option<String>,
        acting: Acting,
        town: Option<TownName>,
        label: String,
        prompt: String,
        started: Box<Started>,
    },
}

pub(super) fn register(node: &Arc<Node>) {
    crate::submitted::register(node, FAMILY, |node: Arc<Node>, asked: Asked| async move {
        match asked {
            Asked::Route { asker, label, prompt, callee, login } => {
                node.residents.active(&asker)?;
                crate::town::access::current(&node, &asker)?;
                let town = node
                    .residents
                    .town_of(&asker)
                    .ok_or_else(|| Failure::not_run(format!("no resident {asker} lives on {}", node.machine())))?;
                crate::colleagues::ask(&node, asker, town, label, prompt, callee, login.as_ref())
                    .await
                    .map(|born| (born.colleague, born.waiting))
            }
            Asked::Here { requester, turn, acting, town, label, prompt, started } => crate::colleagues::ask_here(
                &node,
                crate::colleagues::Requester { address: requester, turn },
                acting,
                town,
                label,
                prompt,
                *started,
            )
            .await
            .map(|(resident, waiting)| (Address { machine: node.machine(), resident }, waiting)),
        }
    });
}

fn unit(asker: &Address) -> String {
    let bytes = serde_json::to_vec(asker).expect("an address serializes");
    format!("ask/{}", fastctx_hands::files::content_hash(&bytes))
}

pub(crate) async fn ask(
    node: &Arc<Node>,
    asker: ResidentId,
    label: String,
    prompt: String,
    callee: Option<String>,
    login: Option<Caller>,
) -> Result<crate::colleagues::Born, Failure> {
    node.residents.active(&asker)?;
    // This caller lives here, so its resident ID stays a unit across machine renames.
    let unit = asker.to_string();
    let (colleague, waiting) =
        crate::submitted::submit(node, &FAMILY, &unit, &Asked::Route { asker, label, prompt, callee, login }).await?;
    Ok(crate::colleagues::Born { colleague, waiting })
}

pub(crate) async fn ask_here(
    node: &Arc<Node>,
    requester: crate::colleagues::Requester,
    acting: Acting,
    town: Option<TownName>,
    label: String,
    prompt: String,
    started: Started,
) -> Result<(ResidentId, Option<u32>), Failure> {
    if !node.is_me(&started.callee.login.machine) {
        return Err(Failure::not_run("the configured callee lives on another machine"));
    }
    crate::logins::workbench::may_start(node, &acting, "codex", started.callee.login.home.as_deref())?;
    let unit = unit(&requester.address);
    let asked = Asked::Here {
        requester: requester.address,
        turn: requester.turn,
        acting,
        town,
        label,
        prompt,
        started: Box::new(started),
    };
    let (colleague, waiting): (Address, Option<u32>) = crate::submitted::submit(node, &FAMILY, &unit, &asked).await?;
    Ok((colleague.resident, waiting))
}
