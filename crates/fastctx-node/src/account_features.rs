//! Account opt-ins are read from the existing versioned sentinel settings record.
use crate::Node;
use fastctx_types::{ResidentId, access::HubAccount, ipc::Failure, sentinel_settings::Settings};
use std::sync::Arc;

pub(crate) async fn read(node: &Node, account: &HubAccount) -> Result<Settings, Failure> {
    // These settings belong to the account, rather than to a shared conversation.
    crate::record_access::within(None, async {
        fastctx_sentinel::settings::read(node.sentinels.records()?, account)
            .await
            .map(|(_, settings)| settings)
            .map_err(|error| Failure::not_run(error.to_string()))
    })
    .await
}

pub(crate) async fn conversation(node: &Arc<Node>, resident: &ResidentId) -> Settings {
    let Some((about, _)) = node.residents.about(resident) else { return Settings::default() };
    let Ok(Some(account)) = crate::plan::load_account(node, resident, &about.acting).await else {
        return Settings::default();
    };
    read(node, &account).await.unwrap_or_default()
}

pub(crate) async fn require_goals(node: &Node, account: &HubAccount) -> Result<(), Failure> {
    if read(node, account).await?.goals { Ok(()) } else { Err(Failure::not_run(fastctx_types::goals::words::DISABLED)) }
}

pub(crate) async fn sentinel(node: &Node, account: &HubAccount, goal: bool) -> Result<bool, Failure> {
    let settings = read(node, account).await?;
    Ok(if goal { settings.goals } else { settings.sentinels })
}

pub(crate) fn changed(node: &Arc<Node>, account: HubAccount) {
    node.tools_changed(None);
    crate::context::invalidations::hub(node, &account.hub);
    node.goals.changed.send_modify(|revision| *revision = revision.wrapping_add(1));
    let node = node.clone();
    fastctx_platform::task::spawn(async move {
        let views: Vec<_> = node
            .goals
            .views
            .lock()
            .expect("goal views")
            .iter()
            .filter(|(_, view)| view.account == account)
            .map(|(id, view)| (id.clone(), view.clone()))
            .collect();
        for (id, view) in views {
            let gate = crate::control_queue::gate(&node, &id);
            let _action = gate.lock().await;
            let _place = node.places.using(id.as_str()).await;
            if let Err(error) = crate::goals::operations::apply(&node, &id, &view).await {
                node.log.line(format_args!("updating {id}'s goal settings: {}", error.message));
            }
        }
    });
}
