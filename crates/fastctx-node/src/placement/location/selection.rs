//! The account or existing receipt a plan-file request selected at its entrance.
use crate::Node;
use fastctx_types::{
    ResidentId,
    access::{Acting, HubAccount},
    ipc::Failure,
    placement::Location,
    residents::Origin,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Clone, Serialize, Deserialize)]
pub(crate) enum Selected {
    Receipt(Option<Location>),
    Account(HubAccount),
    Local(Location),
    Missing,
}
pub(crate) async fn resolve(node: &Arc<Node>, id: &ResidentId, acting: &Acting) -> Result<Selected, Failure> {
    if node.residents.now(id).is_some_and(|now| now.turn.is_some()) {
        let receipt = node.world.conversation(id).path().join("working-place.json");
        return fastctx_platform::task::blocking(move || super::read::<Option<Location>>(&receipt))
            .await
            .map(|location| Selected::Receipt(location.flatten()));
    }
    Ok(match crate::plan::load_account(node, id, acting).await? {
        Some(scope) => Selected::Account(scope),
        None if node.links().is_empty() && matches!(acting, Acting::Here) => {
            Selected::Local(super::here(node, id).await)
        }
        None => Selected::Missing,
    })
}
impl Selected {
    pub(crate) fn check(&self, node: &Node, acting: &Acting) -> Result<(), Failure> {
        match self {
            Self::Account(scope) if !node.serves(acting, &scope.hub, &scope.account) => {
                Err(Failure::refused("the conversation no longer serves its selected working-copy account"))
            }
            Self::Receipt(Some(location)) | Self::Local(location) => {
                node.authorize_machine(acting, &location.machine.clone().into()).map(|_| ())
            }
            _ => Ok(()),
        }
    }
    pub(crate) async fn prepare(
        &self,
        node: &Arc<Node>,
        id: &ResidentId,
        origin: &Origin,
        acting: &Acting,
    ) -> Result<Option<Location>, Failure> {
        self.check(node, acting)?;
        match self {
            Self::Account(scope) => super::prepare_for(node, id, origin, acting, scope).await,
            Self::Receipt(location) => Ok(location.clone()),
            Self::Local(location) => Ok(Some(location.clone())),
            Self::Missing => Ok(None),
        }
    }
}
