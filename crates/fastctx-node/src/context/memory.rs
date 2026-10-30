//! Bodies derived from account records stay in memory on the conversation's
//! host. Persisted context blocks retain only hashes and delivery cursors.

use fastctx_types::{ResidentId, ipc::Failure};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::Mutex};

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Body {
    pub source: String,
    pub text: String,
}
#[derive(Default)]
pub(crate) struct Bodies(Mutex<BTreeMap<(ResidentId, String), Body>>);

impl Bodies {
    pub(crate) fn remove(&self, id: &ResidentId) {
        self.0.lock().expect("never poisoned").retain(|(resident, _), _| resident != id);
    }
    pub(super) fn put(&self, id: &ResidentId, name: &str, source: &str, text: &str) {
        self.0
            .lock()
            .expect("never poisoned")
            .insert((id.clone(), name.into()), Body { source: source.into(), text: text.into() });
    }
    pub(super) fn get(&self, id: &ResidentId, name: &str, source: &str) -> Option<String> {
        self.0
            .lock()
            .expect("never poisoned")
            .get(&(id.clone(), name.into()))
            .filter(|body| body.source == source)
            .map(|body| body.text.clone())
    }
}

pub(crate) async fn for_session(
    node: &std::sync::Arc<crate::Node>,
    session: &str,
) -> Result<serde_json::Value, Failure> {
    let id = node
        .residents
        .resident_of_thread(session)
        .ok_or_else(|| Failure::new("the Claude session has not been paired"))?;
    node.residents.active(&id)?;
    let (about, _) = node.residents.about(&id).ok_or_else(|| Failure::new("unknown conversation"))?;
    if about.host.agent.as_str() != "claude" {
        return Err(Failure::refused("this is not a Claude session"));
    }
    crate::plan::context(node, &id, &about.origin, &about.acting).await?;
    crate::context::sources::skills(node, &id, &about.acting).await?;
    let bodies: BTreeMap<_, _> = node
        .context_bodies
        .0
        .lock()
        .expect("never poisoned")
        .iter()
        .filter(|((resident, _), _)| resident == &id)
        .map(|((_, name), body)| (name.clone(), body.clone()))
        .collect();
    serde_json::to_value(bodies).map_err(|error| Failure::new(error.to_string()))
}
