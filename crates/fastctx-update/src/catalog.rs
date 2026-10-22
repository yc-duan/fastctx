//! Resolves a selected release from device knowledge, GitHub, or its hubs.

use std::sync::Arc;

use fastctx_net::link::Link;
use fastctx_types::update::{Manifest, State, ViewCall, ViewReply};

pub async fn manifest(
    state: &State,
    cached: Vec<Manifest>,
    links: &[Arc<Link>],
    version: &str,
) -> Result<Manifest, String> {
    let known = state
        .offer
        .iter()
        .chain(state.chosen.iter())
        .chain(state.looked.stable.iter())
        .chain(state.looked.beta.iter())
        .chain(state.looked.newer.iter())
        .chain(cached.iter())
        .chain(state.network.iter().flat_map(|n| n.stable.iter().chain(n.beta.iter())))
        .chain(state.network.iter().flat_map(|n| n.due.values().filter_map(|d| d.manifest.as_ref())))
        .chain(state.network.iter().flat_map(|n| n.repairs.values().filter_map(|d| d.manifest.as_ref())))
        .find(|manifest| manifest.version == version);
    if let Some(known) = known {
        return Ok(known.clone());
    }
    let mut why = match crate::github::manifest_of(&state.looked.releases, version).await {
        Ok(manifest) => return Ok(manifest),
        Err(why) => why,
    };
    for link in links {
        if !link.current_versions().is_some_and(|network| network.updates_view) {
            continue;
        }
        let accounts = link.view().me().map(|me| me.logins.clone()).unwrap_or_default();
        for account in accounts {
            match link.network_updates(&account, ViewCall::Release { version: version.into() }).await {
                Ok(ViewReply::Release { choice }) => {
                    if let Some(manifest) = choice.manifest {
                        return Ok(manifest);
                    }
                    if let Some(problem) = choice.problem {
                        why = problem.why;
                    }
                }
                Err(error) => why = error,
                _ => {}
            }
        }
    }
    Err(why)
}
