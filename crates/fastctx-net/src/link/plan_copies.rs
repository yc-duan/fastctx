//! Capability-gated working-copy exchange; old hubs continue serving ordinary plans.

use super::Link;
use crate::wire::{Question, Told};
use fastctx_types::{
    AccountName,
    plan_copies::{Reply, Request},
};

impl Link {
    pub async fn plan_copy(&self, account: &AccountName, request: Request) -> Result<Reply, String> {
        request.validate()?;
        if !self.view().plan_copies {
            return Err("update the hub to synchronize plan working copies and retain their versions".into());
        }
        let question = Question::PlanCopies { account: account.clone(), request };
        match self.inner.ask(question).await? {
            Told::PlanCopies { reply } => Ok(reply),
            _ => Err("the hub did not answer the plan-copy request".into()),
        }
    }
}
