//! Collaboration's first update and complete fixture tree are separate observations.
use super::*;
use fastctx_types::collaboration::Tree;

fn complete(tree: &Tree) -> Result<()> {
    anyhow::ensure!(tree.colleagues.len() == 4, "expected four colleagues, got {}", tree.colleagues.len());
    for branch in std::iter::once(&tree.conversation).chain(tree.colleagues.iter()) {
        anyhow::ensure!(
            branch.unavailable.is_empty(),
            "{} unavailable: {:?}",
            branch.address.resident,
            branch.unavailable
        );
        anyhow::ensure!(
            branch.agents.len() == 2,
            "{}: expected two native children, got {}",
            branch.address.resident,
            branch.agents.len()
        );
        for agent in &branch.agents {
            anyhow::ensure!(agent.unavailable.is_none(), "{} unavailable: {:?}", agent.id, agent.unavailable);
        }
    }
    Ok(())
}

pub(super) async fn query(world: &World, resident: ResidentId) -> Result<Value> {
    let tree = world.collaboration(resident, Some(machine("host").into())).await?;
    complete(&tree)?;
    Ok(serde_json::to_value(tree)?)
}

pub(super) async fn watch(world: &World, resident: ResidentId) -> Result<Value> {
    let began = Instant::now();
    let (mut watch, mut tree) = world.watch_collaboration(resident, Some(machine("host").into())).await?;
    let first_ms = began.elapsed().as_secs_f64() * 1000.0;
    let first_bytes = json_bytes(&tree)?;
    let first_complete = complete(&tree).is_ok();
    while complete(&tree).is_err() {
        tree = watch.next().await?.context("collaboration ended before the complete tree")?;
    }
    Ok(json!({"first_update_ms":first_ms,"first_update_bytes":first_bytes,"first_update_complete":first_complete,
        "complete_ms":began.elapsed().as_secs_f64()*1000.0,"tree":tree}))
}
