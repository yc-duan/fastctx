//! Resource controls retain their acting identity through record reads, CAS and durable recovery.

use fastctx_types::{
    Address,
    access::HubAccount,
    account_records::{Request, Response},
    ipc::Failure,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Access {
    pub actor: HubAccount,
    pub owner: HubAccount,
    pub conversation: Address,
}

tokio::task_local! { static ACCESS: Option<Access>; }

pub(crate) fn current() -> Option<Access> {
    ACCESS.try_with(Clone::clone).ok().flatten()
}

pub(crate) async fn within<F: std::future::Future>(access: Option<Access>, future: F) -> F::Output {
    ACCESS.scope(access, future).await
}

pub(crate) async fn ask(
    link: &fastctx_net::link::Link,
    account: &HubAccount,
    request: Request,
) -> Result<Response, Failure> {
    if let Some(access) = current() {
        if &access.owner != account {
            return Err(Failure::refused("a shared record control cannot read another account"));
        }
        let response = link
            .owned_records(
                &access.actor.account,
                fastctx_types::owned_records::Request::Record { conversation: access.conversation, request },
            )
            .await
            .map_err(Failure::new)?;
        match response {
            fastctx_types::owned_records::Response::Record { response } => Ok(response),
            _ => Err(Failure::new("the hub did not return the conversation's record")),
        }
    } else {
        link.account_records(&account.account, request).await.map_err(Failure::new)
    }
}
