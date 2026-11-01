//! Ordered host projections run on workers and retain their permit through
//! cancellation. SQL dirty markers survive an interrupted file publication.
use super::local;
use crate::Node;
use fastctx_types::ResidentId;
use std::{
    io,
    path::{Path, PathBuf},
    sync::Arc,
};

pub(super) fn exchange(node: &Node, id: &ResidentId) -> Option<PathBuf> {
    node.residents
        .about(id)
        .filter(|(about, _)| about.host.agent.as_str() == "claude")
        .and_then(|(about, _)| about.session)
        .map(|session| fastctx_hosts::claude::exchange::sessions(node.world.path()).join(session))
}
pub(super) async fn repair(node: &Arc<Node>, id: &ResidentId, exchange: Option<PathBuf>) -> io::Result<()> {
    let lease = node.goals.projecting.get_or_insert_with(id, || Arc::new(tokio::sync::Mutex::new(())));
    let permit = Arc::clone(&lease).lock_owned().await;
    let (node, id) = (node.clone(), id.clone());
    fastctx_platform::task::spawn_blocking(move || {
        let _lease = lease;
        let _permit = permit;
        project(&node, &id, exchange.as_deref())
    })
    .await
    .map_err(io::Error::other)?
}
fn project(node: &Node, id: &ResidentId, exchange: Option<&Path>) -> io::Result<()> {
    let directory = node.world.conversation(id);
    crate::conversation_store::with_live_files(&directory, || project_live(node, id, exchange))
}
fn project_live(node: &Node, id: &ResidentId, exchange: Option<&Path>) -> io::Result<()> {
    let directory = node.world.conversation(id);
    loop {
        let (version, bridge) = local::projection(&directory)?;
        if let (Some(exchange), Some(bridge)) = (exchange, bridge)
            && exchange.try_exists()?
        {
            let exchange = fastctx_platform::disk::canonicalize(exchange)?;
            fastctx_hosts::claude::control::write(&exchange.join("goal.json"), &bridge)?;
        }
        if local::projected(&directory, version)? {
            return Ok(());
        }
    }
}
pub(super) fn start(node: &Arc<Node>) {
    let node = node.clone();
    fastctx_platform::task::spawn(async move {
        loop {
            let notified = node.goals.projection_changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let mut after = String::new();
            let mut failed = false;
            loop {
                let (reading, cursor) = (node.clone(), after.clone());
                let rows=fastctx_platform::task::spawn_blocking(move||reading.database.read_now(|db|{
                    let mut query=db.prepare_cached("SELECT conversation FROM goal_projection_dirty WHERE conversation>?1 ORDER BY conversation LIMIT 16")?;
                    Ok(query.query_map([cursor],|row|row.get::<_,String>(0))?.collect::<std::result::Result<Vec<_>,_>>()?)
                })).await;
                let rows = match rows {
                    Ok(Ok(rows)) => rows,
                    other => {
                        failed = true;
                        node.log.line(format_args!("reading goal projections: {other:?}"));
                        break;
                    }
                };
                let Some(last) = rows.last() else { break };
                after = last.clone();
                for key in rows {
                    let id = match key.parse::<ResidentId>() {
                        Ok(id) => id,
                        Err(error) => {
                            failed = true;
                            node.log.line(format_args!("goal projection scope {key}: {error}"));
                            continue;
                        }
                    };
                    if let Err(error) = repair(&node, &id, exchange(&node, &id)).await {
                        failed = true;
                        node.log.line(format_args!("projecting goal for {id}: {error}"));
                    }
                }
            }
            if failed {
                tokio::select! {()=&mut notified=>{},()=tokio::time::sleep(std::time::Duration::from_secs(5))=>{}}
            } else {
                notified.await;
            }
        }
    });
}
