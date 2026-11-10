//! Read receipts retain the exact merge base, including edits not yet on the hub.

use super::{
    bodies,
    state::{Copy, Observed},
    sync,
};
use crate::Node;
use fastctx_types::{Address, ipc::Failure};
use std::{path::Path, sync::Arc};

pub(crate) async fn base(
    node: &Arc<Node>,
    link: &fastctx_net::link::Link,
    copy: &Copy,
    observed: &Observed,
) -> Result<String, String> {
    let hash = observed.hash.as_ref().ok_or("the observed text is unavailable; view the copy again")?;
    let (world, id, expected) = (node.world.clone(), copy.id(), hash.clone());
    match fastctx_platform::task::blocking(move || bodies::read_hash(&world, &id, &expected)).await {
        Ok(text) => Ok(text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let text = super::backend::base(link, copy, observed.version).await?;
            if bodies::Text::of(&text).hash != *hash {
                return Err("the observed base is unavailable; view the latest copy before writing".into());
            }
            sync::keep(node, copy, &text).await?;
            Ok(text)
        }
        Err(error) => Err(error.to_string()),
    }
}

pub(crate) async fn saw(
    node: &Arc<Node>,
    owner: &Address,
    seen: &[fastctx_types::residents::Seen],
) -> Result<(), Failure> {
    let failure = |error: std::io::Error| Failure::new(error.to_string());
    node.replicas.state.load(&node.world).map_err(failure)?;
    for seen in seen {
        let Some(slot) = node.replicas.state.at(Path::new(&seen.path)) else { continue };
        let mut copy = slot.lock().await;
        let shown = copy.effective().clone();
        let same = seen.sha256.as_ref().map_or_else(
            || {
                fastctx_platform::disk::metadata(&copy.path).ok().is_some_and(|metadata| {
                    metadata.len() == seen.size
                        && metadata.len() == shown.len
                        && metadata.modified().ok().and_then(|time| jiff::Timestamp::try_from(time).ok())
                            == seen.modified
                        && fastctx_platform::disk::read(&copy.path)
                            .is_ok_and(|bytes| bodies::Text::of_bytes(&bytes) == shown)
                })
            },
            |seen| *seen == shown.hash,
        );
        let observed = Observed { version: copy.head.version, hash: same.then_some(shown.hash) };
        if copy.observed.get(&owner.key()) == Some(&observed) {
            continue;
        }
        let mut next = copy.clone();
        next.observed.insert(owner.key(), observed);
        super::publication::save(node, &mut next, false, None).await.map_err(Failure::new)?;
        *copy = next;
    }
    Ok(())
}
