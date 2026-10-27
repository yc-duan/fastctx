//! Attribute a memory write to the actual conversation, including remote shell jobs.

use crate::{Node, town::caller::Caller};
use fastctx_types::{
    access::Purpose,
    ipc::Failure,
    memory::Author,
    project::Stamp,
    remote::{RemoteCall, RemoteReply},
    residents::Origin,
};
use std::sync::Arc;

pub(crate) async fn author(node: &Arc<Node>, caller: &Caller) -> Result<Author, Failure> {
    scoped(node, caller, &crate::around::acting_of(node, caller)).await
}

pub(crate) async fn scoped(
    node: &Arc<Node>,
    caller: &Caller,
    acting: &fastctx_types::access::Acting,
) -> Result<Author, Failure> {
    let about = get(node, caller, acting).await?;
    if about.as_ref().is_some_and(|about| matches!(about.origin, Origin::Colleague { .. })) {
        return Err(Failure::refused(fastctx_types::memory::words::COLLEAGUE_READ_ONLY));
    }
    Ok(Author {
        stamp: Stamp { by: caller.writer(), on: node.machine(), at: jiff::Timestamp::now() },
        host: about.as_ref().map(|about| about.host.clone()),
        model: about.as_ref().and_then(|about| about.model.clone()),
        session: about.and_then(|about| about.session),
    })
}

async fn get(
    node: &Arc<Node>,
    caller: &Caller,
    acting: &fastctx_types::access::Acting,
) -> Result<Option<fastctx_types::residents::About>, Failure> {
    Ok(match caller {
        Caller::User => None,
        Caller::Resident(address) if node.is_me(&address.machine) => Some(
            node.residents
                .about(&address.resident)
                .ok_or_else(|| Failure::refused(fastctx_types::memory::words::WRITER_GONE))?
                .0,
        ),
        Caller::Resident(address) => {
            let mut machine = crate::reach::Written::from(&address.machine);
            if let fastctx_types::access::Acting::Accounts { accounts } = acting
                && let [account] = accounts.as_slice()
            {
                machine.hub = node.link_of(&account.hub).map(|link| link.hub_name());
            }
            let reply = node
                .call(acting, &machine, Purpose::Look, RemoteCall::Who { all: true, town: None, since: None })
                .await
                .map_err(|error| Failure::new(error.to_string()))??;
            let RemoteReply::Who(who) = reply else {
                return Err(Failure::new(fastctx_types::memory::words::WRITER_UNANSWERED));
            };
            Some(
                who.residents
                    .into_iter()
                    .find(|resident| resident.about.id == address.resident)
                    .ok_or_else(|| Failure::refused(fastctx_types::memory::words::WRITER_INVISIBLE))?
                    .about,
            )
        }
        _ => return Err(Failure::refused(fastctx_types::memory::words::PRIMARY_WRITER)),
    })
}

pub(crate) async fn read_only(node: &Arc<Node>, caller: &Caller) -> Result<bool, Failure> {
    match caller {
        Caller::User => Ok(false),
        Caller::Resident(_) => Ok(get(node, caller, &crate::around::acting_of(node, caller))
            .await?
            .is_some_and(|about| matches!(about.origin, Origin::Colleague { .. }))),
        _ => Ok(true),
    }
}
