//! A phone reads and edits history directly at the hub, even with all hosts offline.

use super::{Device, failed};
use crate::Error;
use fastctx_types::{
    Address,
    history::{Author, HistoryCall, HistoryReply, Scope},
    project::{Stamp, Writer},
};

impl Device {
    pub(crate) fn history_readers(
        &self,
        scope: &Scope,
    ) -> Vec<(fastctx_types::access::HubId, Vec<fastctx_types::AccountName>)> {
        self.sessions()
            .iter()
            .map(|session| (session.link.hub().clone(), session.link.history_readers(scope)))
            .collect()
    }

    pub(super) async fn history(&self, request: HistoryCall) -> Result<HistoryReply, Error> {
        crate::history::enabled()?;
        let (link, account) = self.history_network(request.scope()).await?;
        let author = request.writes().then(|| Author {
            stamp: Stamp { by: Writer::User, on: Some(link.membership().name), at: jiff::Timestamp::now() },
            host: None,
            model: None,
        });
        let mut reply = link.history_call(&account, request, author).await.map_err(failed)?;
        let view = link.view();
        let usable = |machine: &fastctx_types::MachineName| view.rights(&account, machine).hands;
        match &mut reply {
            HistoryReply::Catalog { catalog } => catalog.for_reader(&usable),
            HistoryReply::Document { document } => document.for_reader(&usable),
            HistoryReply::Saved { revision } => revision.author.stamp.for_reader(&usable),
        }
        Ok(reply)
    }

    pub(crate) async fn history_network(
        &self,
        scope: &Scope,
    ) -> Result<(std::sync::Arc<fastctx_net::link::Link>, fastctx_types::AccountName), Error> {
        Ok(match scope {
            Scope::Project { project } => {
                let (link, account, _) = self.project_scope(project)?;
                (link, account)
            }
            Scope::Conversation { resident, machine } => {
                let address = Address { resident: resident.clone(), machine: machine.clone() };
                let mut found = None;
                let mut older_hub = false;
                for session in self.sessions() {
                    if !session.link.view().shared_records {
                        older_hub = true;
                        continue;
                    }
                    let logins = session.link.view().me().map(|member| member.logins.clone()).unwrap_or_default();
                    for account in logins {
                        if let Ok(fastctx_types::owned_records::Response::Conversation { .. }) = session
                            .link
                            .owned_records(
                                &account,
                                fastctx_types::owned_records::Request::Resolve { conversation: address.clone() },
                            )
                            .await
                        {
                            if found.is_some() {
                                return Err(failed(
                                    "more than one signed-in account has this conversation; select its network",
                                ));
                            }
                            found = Some((session.link.clone(), account));
                        }
                    }
                }
                found.ok_or_else(|| failed(if older_hub {
                    "update the hub to use history"
                } else {
                    "this account has no registered history for that conversation; an older conversation host must be updated"
                }))?
            }
        })
    }
}
