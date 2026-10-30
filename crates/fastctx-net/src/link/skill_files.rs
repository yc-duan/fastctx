//! Capability-gated skill folders. Older hubs keep serving instructions-only skills.

use super::Link;
use crate::wire::{Question, Told};
use fastctx_types::{
    AccountName,
    skill_files::{Reply, Request},
};

impl Link {
    pub async fn skill_files(&self, account: &AccountName, request: Request) -> Result<Reply, String> {
        request.validate()?;
        if !self.view().skill_files {
            return Err("update the hub to read and edit complete skill folders".into());
        }
        match self.inner.ask(Question::SkillFiles { account: account.clone(), request }).await? {
            Told::SkillFiles { reply } => Ok(reply),
            _ => Err("the hub did not answer the skill folder request".into()),
        }
    }
}
