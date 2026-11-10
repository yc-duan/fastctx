//! A legacy observer still needs whole snapshots. Coalesce and reconstruct those
//! from committed storage, outside the editor's locks and commit callback.

use super::*;

#[derive(Default)]
pub(super) struct Refreshes(Mutex<HashMap<ResidentId, u64>>);

pub(super) fn request(node: &Arc<Node>, runtime: &tokio::runtime::Handle, id: &ResidentId, revision: u64) {
    {
        let mut pending = node.composers.legacy_refreshes.0.lock().expect("legacy composer refreshes");
        if let Some(wanted) = pending.get_mut(id) {
            *wanted = (*wanted).max(revision);
            return;
        }
        pending.insert(id.clone(), revision);
    }
    let (weak, id) = (Arc::downgrade(node), id.clone());
    fastctx_platform::task::spawn_on(runtime, async move {
        let Some(node) = weak.upgrade() else {
            return;
        };
        let mut retry = std::time::Duration::from_millis(250);
        loop {
            if !watching::legacy(&node, &id) || node.residents.active(&id).is_err() {
                node.composers.legacy_refreshes.0.lock().expect("legacy composer refreshes").remove(&id);
                return;
            }
            let (view, busy) = match snapshot(&node, &id).await {
                Ok(snapshot) => snapshot,
                Err(error) => {
                    node.log.line(format_args!("reading a legacy composer snapshot: {error}"));
                    tokio::time::sleep(retry).await;
                    retry = (retry * 2).min(std::time::Duration::from_secs(30));
                    continue;
                }
            };
            retry = std::time::Duration::from_millis(250);
            let revision = view.revision;
            if node.residents.active(&id).is_ok() {
                publication::committed(&node, &tokio::runtime::Handle::current(), &id, Arc::new(view), busy);
            }
            {
                let mut pending = node.composers.legacy_refreshes.0.lock().expect("legacy composer refreshes");
                if pending.get(&id).is_none_or(|wanted| *wanted <= revision) {
                    pending.remove(&id);
                    return;
                }
            }
            tokio::task::yield_now().await;
        }
    });
}

/// Full legacy views are read and released outside the editor's locks.
pub(super) async fn snapshot(
    node: &Arc<Node>,
    id: &ResidentId,
) -> Result<(fastctx_types::composer::Composer, bool), Failure> {
    let mut state = persistence::read(node, id).await?;
    node.residents.active(id)?;
    failures::apply(node, id, &mut state);
    watching::restore(node, id, &mut state);
    let busy = idle::busy(&state);
    let view = fastctx_platform::task::spawn_blocking(move || state.view()).await.map_err(failure)?;
    Ok((view, busy))
}
