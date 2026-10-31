//! Conversation records can be read while their agent's host is offline.

use super::Link;
use crate::wire::{Question, Told};
use fastctx_types::{
    AccountName,
    owned_records::{Request, Response},
};

impl Link {
    pub async fn owned_records(&self, account: &AccountName, request: Request) -> Result<Response, String> {
        if !self.view().shared_records {
            return Err("update the hub to use shared conversation records and replicated history".into());
        }
        let connection = self.inner.connected().await.ok_or("the hub is disconnected")?;
        let question = Question::OwnedRecords { account: account.clone(), request };
        let answer = self.inner.ask_on(connection, question).await?.ok_or("the hub disconnected before answering")?;
        match answer {
            Told::OwnedRecords { response } => Ok(response),
            _ => Err("the hub returned an unexpected conversation-record answer".into()),
        }
    }
}
