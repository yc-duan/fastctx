//! A working directory has one durable account incarnation and conversation.
use crate::Node;
use fastctx_types::{Address, access::HubAccount, ipc::Failure};
use serde::{Deserialize, Serialize};
use std::path::Path;
#[derive(Serialize, Deserialize)]
struct Binding {
    account: HubAccount,
    opened: jiff::Timestamp,
    owner: Address,
}
fn failure(error: impl std::fmt::Display) -> Failure {
    Failure::new(error.to_string())
}
fn checked(
    node: &Node,
    directory: &Path,
    scope: &HubAccount,
    owner: &Address,
    create: bool,
) -> Result<Address, Failure> {
    let link = node.link_of(&scope.hub).ok_or_else(|| failure("the working directory's hub is unavailable"))?;
    let view = crate::authorization::current_view(&link)?;
    let opened =
        view.account(&scope.account).ok_or_else(|| failure("the working directory's account is unavailable"))?.opened;
    let path = directory.join(".working-copy-owner.json");
    let binding = Binding { account: scope.clone(), opened, owner: owner.clone() };
    if create {
        fastctx_platform::ensure_private_dir(directory).map_err(failure)?;
        match fastctx_platform::publish_new(
            &path,
            &serde_json::to_vec(&binding).map_err(failure)?,
            fastctx_platform::Permissions::Private,
        ) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(failure(error)),
        }
    }
    let kept: Binding = serde_json::from_slice(&std::fs::read(path).map_err(failure)?).map_err(failure)?;
    if kept.account != *scope
        || kept.opened != opened
        || kept.owner.resident != owner.resident
        || !view.same(&kept.owner.machine, &owner.machine)
    {
        return Err(Failure::refused("the working directory belongs to another account incarnation or conversation"));
    }
    Ok(kept.owner)
}
pub(crate) fn bind(node: &Node, directory: &Path, scope: &HubAccount, owner: &Address) -> Result<Address, Failure> {
    checked(node, directory, scope, owner, true)
}
pub(crate) fn verify(node: &Node, directory: &Path, scope: &HubAccount, owner: &Address) -> Result<(), Failure> {
    checked(node, directory, scope, owner, false).map(drop)
}
