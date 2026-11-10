//! A receipt is still read under the reader's current project and machine rights.
use crate::{Node, admission::Asker, town::caller::Caller};
use fastctx_types::{
    Call, Reply,
    access::Acting,
    ipc::Failure,
    remote::{RemoteCall, RemoteReply},
};

pub(super) fn local(
    node: &Node,
    caller: &Caller,
    call: &Call,
    reply: &mut Reply,
    resolved: &super::Resolved,
) -> Result<(), Failure> {
    let fixed = match resolved {
        super::Resolved::Page { town, .. } | super::Resolved::ProjectEdit { town } => Some(town),
        _ => None,
    };
    filter(node, &crate::around::acting_of(node, caller), call, reply, fixed)
}

pub(super) fn remote(node: &Node, asker: &Asker, call: &RemoteCall, reply: &mut RemoteReply) -> Result<(), Failure> {
    let acting = Acting::one(asker.hub.clone(), asker.account.clone());
    match (call, reply) {
        (RemoteCall::Relay { made }, RemoteReply::Relayed { answer }) => {
            let call: Call = serde_json::from_value(made.clone()).map_err(|error| Failure::new(error.to_string()))?;
            let mut reply: Reply =
                serde_json::from_value(answer.take()).map_err(|error| Failure::new(error.to_string()))?;
            filter(node, &acting, &call, &mut reply, None)?;
            *answer = serde_json::to_value(reply).map_err(|error| Failure::new(error.to_string()))?;
        }
        (RemoteCall::EnterProject { town, .. }, RemoteReply::Entered { map, .. }) => {
            crate::town::access::check(node, &acting, town)?;
            if let Some(usable) = crate::ledger::query::usable(node, &acting, town).map_err(failure)? {
                map.record.for_reader(&*usable);
                map.opened.for_reader(&*usable);
                for page in &mut map.pages {
                    page.current.for_reader(&*usable);
                }
            }
        }
        _ => {}
    }
    Ok(())
}

fn filter(
    node: &Node,
    acting: &Acting,
    call: &Call,
    reply: &mut Reply,
    fixed: Option<&fastctx_types::TownName>,
) -> Result<(), Failure> {
    let town = fixed.or_else(|| match call {
        Call::Page(call) => call.town(),
        Call::Project(fastctx_types::ProjectCall::New { town, .. } | fastctx_types::ProjectCall::Enter { town }) => {
            Some(town)
        }
        Call::Project(fastctx_types::ProjectCall::Edit { town, .. }) => town.as_ref(),
        _ => None,
    });
    let Some(town) = town else { return Ok(()) };
    crate::town::access::check(node, acting, town)?;
    let Some(usable) = crate::ledger::query::usable(node, acting, town).map_err(failure)? else { return Ok(()) };
    match reply {
        Reply::Page { view } => view.for_reader(&*usable),
        Reply::PageConsent { consent, .. } => consent.stamp.for_reader(&*usable),
        Reply::Record { record, opened, .. } => {
            record.for_reader(&*usable);
            opened.for_reader(&*usable);
        }
        Reply::Entered { map, .. } => {
            map.record.for_reader(&*usable);
            map.opened.for_reader(&*usable);
            for page in &mut map.pages {
                page.current.for_reader(&*usable);
            }
        }
        _ => {}
    }
    Ok(())
}
fn failure(error: impl std::fmt::Display) -> Failure {
    Failure::new(error.to_string())
}
