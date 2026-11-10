//! Effectful update work. Reads and repeatable refreshes keep their direct paths;
//! kernel work ends once renewal takes the intent.
mod hubs;
mod machines;
mod transport;
mod uninstall;

use crate::{
    Node,
    submitted::{self, Family},
};
use fastctx_types::{
    AccountName,
    access::HubId,
    ipc::Failure,
    update::{State, UpdateCall},
};
use serde::{Deserialize, Serialize};
use std::{sync::Arc, time::Duration};

pub(crate) use hubs::{arrived, moving, network};
pub(crate) use machines::machine;
pub(super) use transport::call as call_machine;
pub(crate) use uninstall::prepare as prepare_uninstall;

const KERNEL: Family = Family {
    name: "call/update-kernel",
    at_once: 1,
    within: Duration::from_secs(75 * 60),
    // Remote kernel calls have a one-hour caller deadline.
    stays: Duration::from_secs(59 * 60),
    again: false,
};

#[derive(Serialize, Deserialize)]
enum Asked {
    Local { update: UpdateCall },
    Remote { hub: HubId, account: AccountName, update: Option<UpdateCall> },
}

pub(crate) fn register(node: &Arc<Node>) {
    submitted::register(node, KERNEL, |node: Arc<Node>, asked: Asked| async move {
        match asked {
            Asked::Local { update } => super::call(&node, update).await.map_err(Failure::new),
            Asked::Remote { hub, account, update } => {
                // Remote admission checked may_update before acceptance. The
                // accepted work keeps that decision rather than judging it again.
                let asker = super::Asker { hub, by: account, scheduled_at: None };
                super::remote(&node, asker, update).await.map_err(Failure::new)
            }
        }
    });
    machines::register(node);
    hubs::register(node);
    uninstall::register(node);
}

pub(super) fn allowed(update: &UpdateCall) -> Result<(), String> {
    if !fastctx_platform::follows_releases()
        && matches!(
            update,
            UpdateCall::Look | UpdateCall::Fetch { .. } | UpdateCall::Kernel { .. } | UpdateCall::Choose { .. }
        )
    {
        return Err("development builds do not follow releases".into());
    }
    Ok(())
}

fn started(node: &Node) -> bool {
    node.updates.held.lock().expect("never poisoned").started
}

pub(crate) async fn local(node: &Arc<Node>, update: UpdateCall) -> Result<State, Failure> {
    // Recovery may stop before updates::start. The original local path must
    // still be able to hand a replacement to renewal in that degraded state.
    if !started(node) || !matches!(update, UpdateCall::Kernel { .. }) {
        return super::call(node, update).await.map_err(Failure::new);
    }
    let asked = Asked::Local { update };
    submitted::submit_resolving(node, &KERNEL, &asked, || async {
        let Asked::Local { update } = &asked else { unreachable!() };
        allowed(update).map_err(Failure::new)?;
        Ok(("this-machine".to_string(), ()))
    }).await
}

pub(crate) async fn remote(
    node: &Arc<Node>,
    asker: super::Asker,
    update: Option<UpdateCall>,
) -> Result<State, Failure> {
    if !started(node) {
        return Err(Failure::new("update policies have not finished loading; try again"));
    }
    if !matches!(update, None | Some(UpdateCall::Kernel { .. })) {
        return super::remote(node, asker, update).await.map_err(Failure::new);
    }
    let asked = Asked::Remote { hub: asker.hub, account: asker.by, update };
    submitted::submit_resolving(node, &KERNEL, &asked, || async {
        let Asked::Remote { update, .. } = &asked else { unreachable!() };
        let effective = update.clone().unwrap_or(UpdateCall::Kernel { version: None });
        allowed(&effective).map_err(Failure::new)?;
        Ok(("this-machine".to_string(), ()))
    }).await
}

fn unit<T: Serialize>(kind: &str, value: &T) -> String {
    let bytes = serde_json::to_vec(value).expect("submission units serialize");
    format!("{kind}/{}", fastctx_hands::files::content_hash(&bytes))
}
