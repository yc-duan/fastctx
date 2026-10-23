//! Recovery owns no global gate while waiting on account records or a conversation.

use super::intents;
use crate::Node;
use fastctx_types::Address;
use std::sync::Arc;

pub(crate) fn start(node: &Arc<Node>) {
    let node = node.clone();
    tokio::spawn(async move {
        let mut again = crate::again::Again::default();
        loop {
            let loaded = (|| -> std::io::Result<_> {
                let mut owners = Vec::new();
                for kept in node.world.kept()? {
                    for intent in intents::pending(&kept.directory)? {
                        owners.push(intent.resident);
                    }
                }
                Ok(owners)
            })();
            match loaded {
                Ok(owners) => {
                    for owner in owners {
                        schedule(&node, owner);
                    }
                    return;
                }
                Err(error) => node.log.line(format_args!("recovering goal operations: {error}")),
            }
            tokio::time::sleep(again.next()).await;
        }
    });
}

pub(crate) fn schedule(node: &Arc<Node>, resident: Address) {
    if !node.goals.recovering.lock().expect("goal recovery").insert(resident.clone()) {
        return;
    }
    let node = node.clone();
    tokio::spawn(async move {
        let mut again = crate::again::Again::default();
        let mut previous = None;
        loop {
            let result = settle(&node, &resident).await;
            match result {
                Ok(()) => break,
                Err(error) => {
                    if previous.as_ref() != Some(&error.message) {
                        node.log.line(format_args!("recovering {}'s goal: {}", resident.resident, error.message));
                        previous = Some(error.message);
                    }
                }
            }
            tokio::time::sleep(again.next()).await;
        }
    });
}

async fn settle(node: &Arc<Node>, resident: &Address) -> Result<(), fastctx_types::ipc::Failure> {
    let gate = crate::control_queue::gate(node, &resident.resident);
    let _action = gate.lock().await;
    let place = if node.is_me(&resident.machine) { resident.resident.to_string() } else { resident.key() };
    let _place = node.places.using(&place).await;
    let directory = node.world.owner(&node.names(), resident);
    if node.is_me(&resident.machine)
        && super::lifecycle::hold(node, &resident.resident)
            .map_err(|error| fastctx_types::ipc::Failure::new(error.to_string()))?
            .is_some()
    {
        let cached = node.goals.views.lock().expect("goal views").get(&resident.resident).cloned();
        if let Some(view) = cached {
            super::operations::apply(node, &resident.resident, &view).await?;
        } else {
            super::operations::bounded(node.goals.context.set(&resident.resident, "goal", "")).await?;
        }
    }
    for mut intent in
        intents::pending(&directory).map_err(|error| fastctx_types::ipc::Failure::new(error.to_string()))?
    {
        super::operations::finish(node, &mut intent).await?;
    }
    node.goals.recovering.lock().expect("goal recovery").remove(resident);
    Ok(())
}
