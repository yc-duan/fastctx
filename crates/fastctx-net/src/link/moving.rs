//! Moving the hub, as a member asks it (`crate::moving`).

use fastctx_types::AccountName;
use fastctx_types::hub_move::{HubMoveReply, HubMoveRequest};

use super::Link;
use crate::moving::{ArrivalReport, ArrivalRequest, Identity};
use crate::wire::{Question, Told};

impl Link {
    /// Asks the hub for `request` of a move, as `account`, its super
    /// administrator. The view is asked again after a request that changes
    /// the move, so this machine sees it as it stands.
    pub async fn hub_move(&self, account: &AccountName, request: HubMoveRequest) -> Result<HubMoveReply, String> {
        let changes = !matches!(request, HubMoveRequest::Targets | HubMoveRequest::Status);
        // An earlier hub reads the question as a broken frame and ends the
        // session; the view says whether it takes it.
        self.inner.refresh().await?;
        if !self.inner.view().moving {
            return Err("this hub runs a fastctx that cannot be moved; it moves once it runs this one".to_string());
        }
        let told = self.inner.ask(Question::HubMove { account: account.clone(), request }).await?;
        let Told::HubMove { reply } = told else { return Err("the hub answered something else".to_string()) };
        if changes && let Err(error) = self.inner.refresh().await {
            self.inner.node.log(format!("asking the hub for the view after a change to the move: {error}"));
        }
        Ok(reply)
    }

    /// The hub's identity, for the move `id` to this machine.
    pub async fn hub_identity(&self, id: &str) -> Result<Identity, String> {
        match self.inner.ask(Question::HubArrival { request: ArrivalRequest::Take { id: id.to_string() } }).await? {
            Told::HubIdentity { identity } => Ok(*identity),
            _ => Err("the hub answered something else".to_string()),
        }
    }

    /// Tells the hub how this machine stands as the new place of the move `id`.
    pub async fn hub_arrival(&self, id: &str, report: ArrivalReport) -> Result<(), String> {
        let request = ArrivalRequest::Report { id: id.to_string(), report };
        self.inner.ask(Question::HubArrival { request }).await.map(|_| ())
    }
}
