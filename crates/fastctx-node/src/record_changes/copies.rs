//! Working-copy requests keep their resource identity, not an arbitrary worker id.
use crate::{Node, admission::Asker, town::caller::Caller};
use fastctx_types::{
    Address,
    access::{Acting, HubAccount},
    ipc::Failure,
    placement::Resource,
};
use std::{path::PathBuf, sync::Arc};

pub(super) fn paths_check(node: &Node, caller: &Caller, paths: &[PathBuf]) -> Result<String, Failure> {
    node.authorize_here(&crate::around::acting_of(node, caller))?;
    let mut paths: Vec<_> = paths.iter().map(|path| path.to_string_lossy().replace('\\', "/")).collect();
    paths.sort();
    paths.dedup();
    super::unit(&("repair", paths))
}

pub(super) fn check(
    node: &Arc<Node>,
    asker: &Asker,
    owner: &Address,
    placement: u64,
    resource: &Resource,
) -> Result<String, Failure> {
    let scope = HubAccount { hub: asker.hub.clone(), account: asker.account.clone() };
    let link = node.link_of(&scope.hub).ok_or_else(|| Failure::refused("the working-copy hub is unavailable"))?;
    let view = crate::authorization::current_view(&link)?;
    node.authorize_machine(&Acting::one(scope.hub.clone(), scope.account.clone()), &node.machine().into())?;
    if view.shared_records {
        if placement != 0 {
            return Err(Failure::not_run("update the conversation's host to use replicated records"));
        }
    } else {
        if !crate::placement::owned(view.member(&node.machine()), &scope.account) {
            return Err(Failure::refused("working copies belong on the account's own computers"));
        }
        if matches!(resource, Resource::History { .. }) {
            return Err(Failure::not_run("update the hub to use history"));
        }
    }
    match resource {
        Resource::Skill { name, .. } => {
            if !fastctx_types::skills::valid_name(name) {
                return Err(Failure::not_run("invalid skill name"));
            }
            if !crate::placement::owned(view.member(&node.machine()), &scope.account) {
                return Err(Failure::refused("skill folders belong on the account's own working computers"));
            }
            super::unit(&("skill", scope, name))
        }
        Resource::Files { key, files } => {
            crate::placement::artifacts::validate_key(key)?;
            fastctx_types::skill_files::validate_files(files).map_err(Failure::not_run)?;
            super::unit(&("artifact", scope, owner, key))
        }
        Resource::History { scope: history } => {
            let home = view.me().and_then(|member| member.home.as_ref());
            if !home.is_some_and(|account| crate::history::switch::on(&link, account)) {
                return Err(Failure::not_run(crate::history::switch::COPIES_OFF));
            }
            match history {
                fastctx_types::history::Scope::Project { project } => {
                    let project = view
                        .project_named(&asker.account, project)
                        .ok_or_else(|| Failure::refused("this account cannot use the project's history"))?;
                    if !view.member_keeps_project(&view.me, &project.town) {
                        return Err(Failure::refused("this computer does not keep that project's records"));
                    }
                    super::unit(&("project-history", scope.hub, &project.town))
                }
                _ => super::unit(&("copy", scope, owner)),
            }
        }
        _ => super::unit(&("copy", scope, owner)),
    }
}
