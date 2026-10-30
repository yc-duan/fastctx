//! Resolve a skill through the conversation's pinned working place.
use super::*;
use fastctx_types::{
    access::{HubAccount, Purpose},
    placement::{Location, Resource},
    remote::{RemoteCall, RemoteReply},
    skill_files::{Reply, Request},
};

pub(super) async fn locate(
    node: &Arc<Node>,
    caller: &Caller,
    link: &Arc<Link>,
    account: &AccountName,
    name: &str,
) -> Result<(Skill, Option<Location>), Failure> {
    let scope = HubAccount { hub: link.membership().id, account: account.clone() };
    let owner = match caller {
        Caller::Resident(owner) => Some(owner.clone()),
        Caller::User => None,
        _ => return Err(Failure::refused("skill folders are read by an account's conversation or user")),
    };
    if let Some(owner) = &owner
        && node.is_me(&owner.machine)
    {
        let (about, _) =
            node.residents.about(&owner.resident).ok_or_else(|| Failure::new("the conversation is unavailable"))?;
        crate::placement::location::prepare(node, &owner.resident, &about.origin, &about.acting).await?;
    }
    let asker = crate::admission::Asker {
        machine: node.machine(),
        hub: scope.hub.clone(),
        account: scope.account.clone(),
        hands: true,
    };
    let Some(owner) = owner else {
        return copies::prepare(node, &asker, None, name, None).await.map(|(skill, path)| (skill, Some(path)));
    };
    let (placement, place) = crate::placement::location::read_place(link, &scope, &owner).await?;
    let placement = placement.ok_or_else(|| Failure::not_run("the conversation has no working-copy location yet"))?;
    let machine = place
        .and_then(|place| place.machine)
        .ok_or_else(|| Failure::not_run("there is no online working computer for this skill folder"))?;
    node.authorize_machine(&Acting::one(scope.hub.clone(), scope.account.clone()), &machine.clone().into())?;
    let snapshot = match link
        .skill_files(account, Request::Read { name: name.into() })
        .await
        .map_err(|error| Failure::new(error.to_string()))?
    {
        Reply::Read { snapshot } => snapshot,
        _ => return Err(Failure::new("the hub did not return the skill")),
    };
    let skill = snapshot.skill.ok_or_else(|| Failure::not_run(skills::words::missing(name)))?;
    let version = snapshot.metadata.ok_or_else(|| Failure::new("the skill has no version"))?.version;
    let resource = Resource::Skill { name: name.into(), version };
    let location = if node.is_me(&machine) {
        crate::placement::prepare_copy(node, &asker, owner, placement, resource).await?
    } else {
        let facts = link
            .call(&machine, account, Purpose::Look, RemoteCall::Facts)
            .await
            .map_err(|error| Failure::new(error.to_string()))??;
        if !matches!(facts,RemoteReply::Facts(facts) if facts.plan_copies) {
            return Err(Failure::not_run("update the working computer to use skill folders"));
        }
        match link
            .call(&machine, account, Purpose::Hands, RemoteCall::WorkingCopy { owner, placement, resource })
            .await
            .map_err(|error| Failure::new(error.to_string()))??
        {
            RemoteReply::WorkingCopy { location } => location,
            _ => return Err(Failure::new("the working computer did not return the skill path")),
        }
    };
    Ok((skill, Some(location)))
}
