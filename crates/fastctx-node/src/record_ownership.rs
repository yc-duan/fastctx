//! Conversation ownership, shared by visibility, admission and replica placement.

use crate::Node;
use fastctx_types::{
    AccountName, ResidentId,
    access::{Acting, HubId},
    residents::About,
};

/// Isolation at birth survives later grants and withdrawals. Legacy local
/// sessions belong to their computer; ambiguous remote births stay private.
pub(crate) fn shared(node: &Node, about: &About) -> bool {
    match crate::assists::host::kept(node, &about.id) {
        Ok(Some(assisted)) => !assisted,
        Ok(None) => matches!(about.acting, Acting::Here),
        Err(_) => false,
    }
}

pub(crate) fn origin(node: &Node, about: &About) -> Option<fastctx_types::MachineName> {
    if !shared(node, about) {
        return None;
    }
    node.residents.birth_machine(&about.id)
}

pub(crate) fn allows(node: &Node, about: &About, hub: &HubId, account: &AccountName) -> bool {
    let Some(link) = node.link_of(hub) else { return false };
    let view = link.view();
    let accounts = node.accounts_on(&link, &about.acting);
    fastctx_net::record_ownership::allows(&view.directory, account, &accounts, origin(node, about).as_ref())
}

pub(crate) fn resident(node: &Node, id: &ResidentId, hub: &HubId, account: &AccountName) -> bool {
    node.residents.about(id).is_some_and(|(about, _)| allows(node, &about, hub, account))
}

/// What each hub was told of each conversation here, on which connection:
/// told again only when what would be told changes, or on a new connection.
/// A conversation's records are registered at every full pass over the
/// replicas and before every history call, and the hub keeps what it is told.
#[derive(Default)]
pub(crate) struct Registered(std::sync::Mutex<std::collections::HashMap<(HubId, ResidentId), Told>>);

#[derive(PartialEq)]
struct Told {
    connection: jiff::Timestamp,
    account: AccountName,
    shared: bool,
    previous: Option<fastctx_types::MachineName>,
}

/// Registration contains coordinates and birth mode only, never document text.
pub(crate) async fn register(
    node: &std::sync::Arc<Node>,
    resident: &ResidentId,
    acting: &Acting,
) -> Result<(), fastctx_types::ipc::Failure> {
    let Some(scope) = crate::placement::account(node, acting, None)? else { return Ok(()) };
    let Some(link) = node.link_of(&scope.hub) else { return Ok(()) };
    if !link.view().shared_records {
        return Ok(());
    }
    let kept = {
        let (reading, whose) = (node.clone(), resident.clone());
        fastctx_platform::task::blocking(move || crate::assists::host::kept(&reading, &whose)).await?
    };
    let shared = match kept {
        Some(assisted) => !assisted,
        None => matches!(acting, Acting::Here),
    };
    let previous = node.residents.birth_machine(resident);
    let key = (scope.hub.clone(), resident.clone());
    let told = match link.status() {
        fastctx_net::link::Status::Online { since, .. } => {
            Some(Told { connection: since, account: scope.account.clone(), shared, previous: previous.clone() })
        }
        _ => None,
    };
    if told.is_some() && node.registered.0.lock().expect("never poisoned").get(&key) == told.as_ref() {
        return Ok(());
    }
    link.owned_records(
        &scope.account,
        fastctx_types::owned_records::Request::Register { resident: resident.clone(), shared, previous },
    )
    .await
    .map_err(fastctx_types::ipc::Failure::new)?;
    if let Some(told) = told {
        node.registered.0.lock().expect("never poisoned").insert(key, told);
    }
    Ok(())
}

pub(crate) async fn project_origin(
    node: &Node,
    caller: &crate::town::caller::Caller,
    owner: Option<&fastctx_types::access::HubAccount>,
) -> Result<Option<fastctx_types::MachineName>, fastctx_types::ipc::Failure> {
    use crate::town::caller::Caller;
    Ok(match caller {
        Caller::User => Some(node.machine()),
        Caller::Resident(address) if node.is_me(&address.machine) => {
            node.residents.about(&address.resident).and_then(|(about, _)| origin(node, &about))
        }
        Caller::Resident(address) => {
            let scope = owner.ok_or_else(|| {
                fastctx_types::ipc::Failure::refused("a remote conversation needs an account for its project")
            })?;
            let link = node
                .link_of(&scope.hub)
                .ok_or_else(|| fastctx_types::ipc::Failure::new("the project's hub is unavailable"))?;
            let response = link
                .owned_records(
                    &scope.account,
                    fastctx_types::owned_records::Request::Resolve { conversation: address.clone() },
                )
                .await
                .map_err(fastctx_types::ipc::Failure::new)?;
            let fastctx_types::owned_records::Response::Conversation { conversation } = response else {
                return Err(fastctx_types::ipc::Failure::new("the conversation ownership was not returned"));
            };
            conversation.shared_on
        }
        _ => None,
    })
}
