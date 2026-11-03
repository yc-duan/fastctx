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

impl Link {
    /// Resolve record authority independently of the possibly stale activity feed.
    pub async fn record_actor(
        &self,
        address: &fastctx_types::Address,
    ) -> Result<(AccountName, fastctx_types::Address), String> {
        if !self.view().shared_records {
            return self.account_of(&address.machine, &address.resident).map(|account| (account, address.clone()));
        }
        let view = self.view();
        let mut logins: Vec<_> = view.me().into_iter().flat_map(|member| member.logins.iter().cloned()).collect();
        logins.sort_by_key(|account| view.me().and_then(|member| member.home.as_ref()) != Some(account));
        let mut found: Option<(AccountName, fastctx_types::owned_records::Conversation)> = None;
        let mut unregistered = false;
        for actor in logins {
            match self.owned_records(&actor, Request::Resolve { conversation: address.clone() }).await {
                Ok(Response::Conversation { conversation }) => {
                    if found.as_ref().is_some_and(|(_, other)| other != &conversation) {
                        return Err("more than one signed-in account has a different record at this conversation address"
                            .into());
                    }
                    if found.is_none() {
                        found = Some((actor, conversation));
                    }
                }
                Ok(Response::Unregistered) => unregistered = true,
                _ => {}
            }
        }
        // A machine from before the records registered none: the
        // conversation's records are its account's, as before them.
        if found.is_none() && unregistered {
            return self.account_of(&address.machine, &address.resident).map(|account| (account, address.clone()));
        }
        found.map(|(actor, conversation)| (actor, conversation.address)).ok_or_else(|| {
            "this account has no registered record of that conversation; update an older conversation host".into()
        })
    }

    /// Whether the machine of the conversation `whose` registered no
    /// conversation: one from before the records, whose plan and other
    /// records are its account's own.
    pub(crate) async fn unregistered(&self, account: &AccountName, whose: &fastctx_types::Address) -> bool {
        matches!(
            self.owned_records(account, Request::Resolve { conversation: whose.clone() }).await,
            Ok(Response::Unregistered)
        )
    }

    /// A local deletion must retain its hub-owned audience before erasing the ownership record.
    pub async fn prepare_forget(
        &self,
        actor: &AccountName,
        conversation: &fastctx_types::Address,
    ) -> Result<(), String> {
        match self.owned_records(actor, Request::PrepareDeletion { conversation: conversation.clone() }).await? {
            Response::DeletionPrepared => Ok(()),
            _ => Err("the hub did not retain the deletion intent".into()),
        }
    }
}
