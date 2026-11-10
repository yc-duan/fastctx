//! Pending local originals retain the same opener and shared-birth ownership as the hub.

use crate::Node;
use fastctx_types::{access::Acting, history::Scope, ipc::Failure};

pub(crate) fn authorize(node: &Node, acting: &Acting, scope: &Scope) -> Result<(), Failure> {
    let Scope::Conversation { resident, .. } = scope else {
        let Scope::Project { project } = scope else { unreachable!() };
        crate::town::access::check(node, acting, project)?;
        let Some(target) =
            crate::ledger::carry::target(&node.world, project).map_err(|error| Failure::new(error.to_string()))?
        else {
            return Ok(());
        };
        let refused = || Failure::refused("this account cannot use that project's importing history");
        let link = node.link_of(&target.place.hub).ok_or_else(refused)?;
        let view = link.view();
        if view.account(&target.account).is_none_or(|account| account.opened != target.opened) {
            return Err(refused());
        }
        let registered = view.project(&target.place.town).is_some();
        return if node
            .accounts_on(&link, acting)
            .iter()
            .any(|actor| if registered { view.in_project(actor, &target.place.town) } else { actor == &target.account })
        {
            Ok(())
        } else {
            Err(refused())
        };
    };
    let refused = || Failure::refused("this account cannot use that conversation's history");
    let (about, _) = node.residents.about(resident).ok_or_else(refused)?;
    let records = super::super::local::directory(node, scope);
    let transfer = super::state::read(&records).map_err(|error| Failure::new(error.to_string()))?;
    let target = match &transfer {
        Some(transfer) => Some(transfer.account.clone()),
        None => crate::placement::account(node, &about.acting, None)?,
    };
    let Some(target) = target else {
        return if matches!((acting, &about.acting), (Acting::Here, Acting::Here)) { Ok(()) } else { Err(refused()) };
    };
    let link = node.link_of(&target.hub).ok_or_else(refused)?;
    let view = link.view();
    if transfer
        .as_ref()
        .is_some_and(|transfer| view.account(&target.account).is_none_or(|account| account.opened != transfer.opened))
    {
        return Err(refused());
    }
    let birth = transfer.as_ref().and_then(|transfer| transfer.conversation.as_ref()).and_then(|conversation| {
        let machine = conversation.shared_on.as_ref()?;
        let seal = view.directory.member(machine).and_then(fastctx_net::record_ownership::seal)?;
        (conversation.born_seal.as_deref() == Some(seal.as_str())).then_some(machine)
    });
    if node.accounts_on(&link, acting).iter().any(|actor| {
        fastctx_net::record_ownership::allows(&view.directory, actor, std::slice::from_ref(&target.account), birth)
    }) {
        Ok(())
    } else {
        Err(refused())
    }
}
