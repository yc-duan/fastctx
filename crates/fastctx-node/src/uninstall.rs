use crate::Node;
use fastctx_types::ipc::Failure;
use std::sync::Arc;

pub(crate) async fn prepare(node: &Arc<Node>) -> Result<(), Failure> {
    node.handoff.pause_for_uninstall().await;
    let mut failures = Vec::new();
    for resident in node.residents.everyone() {
        if resident.removed.is_some() {
            continue;
        }
        let about = resident.about;
        if !about.world_started || about.host.agent.as_str() != "claude" {
            continue;
        }
        let Some(session) = about.session else {
            failures.push(format!("{} has no published Claude session identity", about.id));
            continue;
        };
        match crate::claude::management::background_session(node, about.home.as_deref(), &session).await {
            Ok(None) => {}
            Ok(Some(_)) => {
                if let Err(error) =
                    crate::lifecycle::release(node, &about.id, crate::lifecycle::ReleaseReason::Uninstall).await
                {
                    failures.push(format!("{}: {error}", about.id));
                }
            }
            Err(error) => failures.push(format!("{}: {error}", about.id)),
        }
    }
    let links = node.links.read().expect("never poisoned").values().cloned().collect::<Vec<_>>();
    for link in links {
        link.stop();
    }
    if !failures.is_empty() {
        return Err(Failure::new(failures.join("; ")));
    }
    Ok(())
}
