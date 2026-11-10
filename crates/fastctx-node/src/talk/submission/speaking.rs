//! Speaking jobs share their target conversation's unit and queue.

use std::sync::Arc;
use std::time::Duration;

use fastctx_types::access::Written;
use fastctx_types::attachments::Attachment;
use fastctx_types::collab::Caller;
use fastctx_types::ipc::Failure;
use fastctx_types::residents::Landed;
use fastctx_types::{Address, MachineName, ResidentId};
use serde::{Deserialize, Serialize};

use crate::Node;

const FAMILY: crate::submitted::Family = crate::submitted::Family {
    name: "call/speak",
    at_once: 16,
    within: Duration::from_secs(300),
    stays: Duration::from_secs(240),
    // Repeating a handover may repeat the user's or another resident's words.
    again: false,
};

#[derive(Serialize, Deserialize)]
enum Asked {
    Talk {
        to: ResidentId,
        machine: Option<Written>,
        text: String,
        attachments: Vec<Attachment>,
        files_on: Option<Written>,
        id: Option<String>,
    },
    Here {
        typed_on: MachineName,
        to: ResidentId,
        text: String,
        attachments: Vec<Attachment>,
        files_on: Option<Written>,
        id: Option<String>,
    },
    Message {
        from: ResidentId,
        to: ResidentId,
        machine: Option<Written>,
        text: String,
        login: Option<Caller>,
    },
}

pub(super) fn register(node: &Arc<Node>) {
    crate::submitted::register(node, FAMILY, |node: Arc<Node>, asked: Asked| async move {
        match asked {
            Asked::Talk { to, machine, text, attachments, files_on, id } => {
                crate::talk::talk(&node, to, machine, text, attachments, files_on, id).await
            }
            Asked::Here { typed_on, to, text, attachments, files_on, id } => {
                crate::talk::talk_here(&node, &typed_on, &to, text, attachments, files_on.as_ref(), id.as_deref())
                    .await
                    .map(|landed| (Address { machine: node.machine(), resident: to }, landed))
            }
            Asked::Message { from, to, machine, text, login } => {
                node.residents.active(&from)?;
                crate::town::access::current(&node, &from)?;
                crate::colleagues::message_work(&node, from, to, machine, text, login.as_ref()).await
            }
        }
    });
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn talk(
    node: &Arc<Node>,
    to: ResidentId,
    machine: Option<Written>,
    text: String,
    attachments: Vec<Attachment>,
    files_on: Option<Written>,
    id: Option<String>,
) -> Result<(Address, Landed), Failure> {
    if machine.as_ref().is_none_or(|machine| node.is_here(machine)) && node.residents.about(&to).is_some() {
        node.residents.active(&to)?;
    }
    let unit = to.to_string();
    crate::submitted::submit(node, &FAMILY, &unit, &Asked::Talk { to, machine, text, attachments, files_on, id }).await
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn talk_here(
    node: &Arc<Node>,
    typed_on: &MachineName,
    to: &ResidentId,
    text: String,
    attachments: Vec<Attachment>,
    files_on: Option<&Written>,
    id: Option<&str>,
) -> Result<Landed, Failure> {
    node.residents.active(to)?;
    let asked = Asked::Here {
        typed_on: typed_on.clone(),
        to: to.clone(),
        text,
        attachments,
        files_on: files_on.cloned(),
        id: id.map(str::to_string),
    };
    let (_, landed): (Address, Landed) = crate::submitted::submit(node, &FAMILY, to.as_str(), &asked).await?;
    Ok(landed)
}

pub(crate) async fn message_here(
    node: &Arc<Node>,
    from: ResidentId,
    to: ResidentId,
    machine: Option<Written>,
    text: String,
    login: Option<Caller>,
) -> Result<(Address, Landed), Failure> {
    node.residents.active(&from)?;
    node.residents.active(&to)?;
    let unit = to.to_string();
    crate::submitted::submit(node, &FAMILY, &unit, &Asked::Message { from, to, machine, text, login }).await
}
