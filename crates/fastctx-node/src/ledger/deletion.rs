//! Finish the hub's explicit project deletions before letting go of ledgers.

use std::sync::Arc;

use fastctx_net::link::Link;
use fastctx_types::management::{Management, ManagementOutcome};

use crate::Node;

pub(crate) async fn finish(node: &Arc<Node>, link: &Arc<Link>) -> Result<(), String> {
    let hub = link.membership().id.clone();
    for project in link.view().project_deletions.clone() {
        let place = super::names::place(&node.world, &project.name).map_err(|error| error.to_string())?;
        if place.is_some_and(|place| place.hub == hub && place.town == project.town) {
            let residents: Vec<_> =
                node.residents.everyone().filter(|resident| resident.town.as_ref() == Some(&project.name)).collect();
            for resident in residents {
                if node.serves(&resident.about.acting, &hub, &project.owner) {
                    crate::forget::project(node, &resident.id, &hub, &project.owner).await?;
                } else if resident.removed.is_some() {
                    super::letgo::leave_kept(node, &resident.id).await.map_err(|error| error.message)?;
                } else {
                    let moved = crate::management::here(node, &resident.id, Management::Move { town: None })
                        .await
                        .map_err(|error| error.message)?;
                    if moved.town.is_some() || moved.steps.iter().any(|step| step.outcome == ManagementOutcome::Failed)
                    {
                        return Err(format!(
                            "moving {} out of deleted project {}: {:?}",
                            resident.id, project.name, moved.steps
                        ));
                    }
                }
            }
            super::letgo::remove(node, &project.name).await.map_err(|error| error.to_string())?;
        }
        link.project_deleted(&project.town).await?;
    }
    Ok(())
}
