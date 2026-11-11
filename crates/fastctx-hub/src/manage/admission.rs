//! Read-only admission for management work; the catalog edit repeats every check.

use super::*;

pub(super) fn check(
    hub: &Hub,
    origin: &crate::origin::Origin,
    account: &AccountName,
    change: &Change,
) -> Result<(), String> {
    let store = hub.store();
    let directory = &store.book.directory;
    origin.check(directory)?;
    let found = directory.account(account).ok_or_else(|| format!("there is no account {account}"))?;
    if found.locked {
        return Err(format!("the account {account} is locked"));
    }
    if !may_change(&store.indexed(), change, origin, account) {
        return Err(format!("{origin} is not logged in to the account {account}"));
    }
    let super_ = found.level == Level::Super;
    let need_super =
        || if super_ { Ok(()) } else { Err(format!("the account {account} is not the hub's super administrator")) };
    match change {
        Change::DeleteAccount { name } => {
            need_super()?;
            let found = directory.account(name).ok_or_else(|| format!("there is no account {name}"))?;
            if found.level == Level::Super {
                return Err(format!(
                    "{name} is the hub's super administrator and cannot be deleted; hand that to another account first"
                ));
            }
        }
        Change::RenameGroup { owner, from, to } => {
            if owner != account && (!super_ || directory.is_visitor(owner)) {
                return Err(format!("the machines of {owner} are not the account {account}'s to group"));
            }
            to.as_deref().map(fastctx_types::accounts::group_name).transpose()?;
            if !store
                .indexed()
                .owned(owner)
                .iter()
                .any(|&at| directory.members[at].group.as_deref() == Some(from.as_str()))
            {
                return Err(format!("{owner} has no group {from}"));
            }
        }
        Change::DeleteProject { town } => {
            let project = directory
                .projects
                .iter()
                .find(|project| &project.town == town)
                .ok_or_else(|| format!("there is no project {town}"))?;
            if &project.owner != account {
                return Err(format!("the project {} is the account {}'s", project.name, project.owner));
            }
            for member in directory
                .members
                .iter()
                .filter(|member| member.is_machine() && directory.member_in_project(&member.name, town))
            {
                if !store.letters.get(&member.name).is_some_and(|said| said.conversation_deletions) {
                    return Err(format!("update {} before deleting a project with conversations there", member.name));
                }
            }
        }

        Change::SetRole { name, visitor } => {
            need_super()?;
            let found = directory.account(name).ok_or_else(|| format!("there is no account {name}"))?;
            match (found.level, *visitor) {
                (Level::Super, _) => {
                    return Err(format!("{name} is the hub's super administrator; hand that to a member first"));
                }
                (Level::Member, false) => return Err(format!("{name} is a member already")),
                _ => {}
            }
        }
        _ => return Err("this management change is not submitted work".into()),
    }
    Ok(())
}

/// Admission failures retain the original refused management audit without
/// accepting a durable operation or running any of the change itself.
pub(super) async fn refused(
    hub: &Arc<Hub>,
    origin: crate::origin::Origin,
    account: AccountName,
    change: Change,
    reason: String,
) -> String {
    hub.blocking(move |hub| {
        let mut store = hub.edit_store();
        if let Err(why) = origin.check(&store.book.directory) {
            return why;
        }
        let action = crate::machine_log::action(&change, &store.book, &store.hub.updates);
        let actor = Actor {
            account: Some(account.clone()),
            authenticated: may_change(&store.indexed(), &change, &origin, &account),
            member: Some(origin.name),
            peer: None,
        };
        let directory = store.book.directory.clone();
        let kept = crate::machine_log::keep(
            &mut store,
            &directory,
            actor,
            action,
            Outcome::Refused { reason: reason.clone() },
        );
        hub.machine_log_changed.notify_one();
        kept.err().unwrap_or(reason)
    })
    .await
}
