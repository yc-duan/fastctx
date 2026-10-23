//! Choose storage before consuming a file, using the conversation's account.

use super::*;
use fastctx_types::access::HubAccount;
use fastctx_types::attachments::AttachmentPlaces;

pub(crate) async fn query(
    node: &Arc<Node>,
    target: UploadTarget,
    machine: Option<Written>,
    account: Option<HubAccount>,
    preferred: Option<Written>,
) -> Result<AttachmentPlaces, Failure> {
    if machine.is_none() && matches!(target, UploadTarget::New { .. }) {
        let acting = local_actor(node, account.as_ref())?;
        if matches!(acting, Acting::Here) {
            return Ok(AttachmentPlaces {
                machines: vec![node.machine().into()],
                selected: Some(node.machine().into()),
                images_only: false,
            });
        }
        return owned(node, &node.machine(), &target, &acting, preferred.as_ref());
    }
    let machine = match &target {
        UploadTarget::Sent { machine, .. } => machine.clone(),
        _ => machine.unwrap_or_else(|| node.machine().into()),
    };
    let acting = sender(node, &machine, &target, account.as_ref())?;
    if node.is_here(&machine) {
        return places(node, &node.machine(), &target, &acting, preferred.as_ref());
    }
    let (link, account) = node.speaking(&acting, &machine, Purpose::Agents).map_err(failure)?;
    match link.call(&machine.name, &account, Purpose::Look, RemoteCall::Facts).await.map_err(broke)?? {
        RemoteReply::Facts(facts) if facts.attachment_places => {}
        _ => return Err(Failure::not_run(format!("upgrade {machine} to choose where attachments are stored"))),
    }
    match link
        .call(&machine.name, &account, Purpose::Agents, RemoteCall::AttachmentPlaces { target, preferred })
        .await
        .map_err(broke)??
    {
        RemoteReply::AttachmentPlaces(places) => Ok(places),
        _ => Err(failure("the conversation machine did not list attachment destinations")),
    }
}

pub(crate) fn local_actor(node: &Node, selected: Option<&HubAccount>) -> Result<Acting, Failure> {
    let Some(selected) = selected else {
        let mut accounts = Vec::new();
        for link in node.links() {
            let view = link.view();
            for account in node.accounts_on(&link, &Acting::Here) {
                if view.account(&account).is_some_and(|account| !account.locked) {
                    accounts.push(HubAccount { hub: view.hub.clone(), account });
                }
            }
        }
        accounts.sort();
        accounts.dedup();
        return match accounts.len() {
            0 => Ok(Acting::Here),
            1 => Ok(Acting::Accounts { accounts }),
            _ => Err(Failure::not_run("choose the account this upload belongs to")),
        };
    };
    let link = node.link_of(&selected.hub).ok_or_else(|| failure("the upload account is not connected"))?;
    let view = link.view();
    if !view.logged_in(&view.me, &selected.account)
        || view.account(&selected.account).is_none_or(|account| account.locked)
    {
        return Err(Failure::refused("this device is not logged in to the upload account"));
    }
    Ok(Acting::one(selected.hub.clone(), selected.account.clone()))
}

pub(crate) fn sender(
    node: &Node,
    machine: &Written,
    target: &UploadTarget,
    selected: Option<&HubAccount>,
) -> Result<Acting, Failure> {
    if selected.is_some() {
        return local_actor(node, selected);
    }
    if let UploadTarget::Resident { resident } = target.conversation_target() {
        if node.is_here(machine) {
            return acting(node, target, &Acting::Here);
        }
        let mut accounts = Vec::new();
        for link in node.locate(machine)? {
            if let Ok(account) = link.account_of(&machine.name, resident) {
                accounts.push(HubAccount { hub: link.membership().id, account });
            }
        }
        if accounts.len() == 1 {
            return Ok(Acting::Accounts { accounts });
        }
    }
    local_actor(node, None)
}

pub(super) fn owns_here(node: &Node, acting: &Acting) -> Result<(), Failure> {
    if matches!(acting, Acting::Here) {
        return Ok(());
    }
    for link in node.links() {
        let view = link.view();
        if node.accounts_on(&link, acting).iter().any(|account| view.holds(account, &view.me)) {
            return Ok(());
        }
    }
    Err(Failure::not_run("upgrade the conversation machine to forward files to this account's own machine"))
}

pub(super) fn acting(node: &Node, target: &UploadTarget, uploader: &Acting) -> Result<Acting, Failure> {
    match target.conversation_target() {
        UploadTarget::Resident { resident } => {
            let (about, _) = node.residents.about(resident).ok_or_else(|| crate::forget::gone(node, resident))?;
            Ok(about.acting)
        }
        UploadTarget::Draft { resident, .. } => {
            let draft: Draft = read_json(&node.world.conversation(resident).draft())?;
            if draft.acting.as_ref() != Some(uploader) {
                return Err(Failure::refused("the upload draft belongs to another account; attach the file again"));
            }
            Ok(uploader.clone())
        }
        _ => Ok(uploader.clone()),
    }
}

pub(super) fn assisted(node: &Node, target: &UploadTarget, acting: &Acting) -> Result<bool, Failure> {
    let isolated = match target.conversation_target() {
        UploadTarget::Resident { resident } => crate::assists::host::kept(node, resident)? == Some(true),
        _ => false,
    };
    Ok(isolated || !crate::around::works_here(node, acting))
}

pub(crate) fn places(
    node: &Node,
    from: &MachineName,
    target: &UploadTarget,
    uploader: &Acting,
    preferred: Option<&Written>,
) -> Result<AttachmentPlaces, Failure> {
    let acting = acting(node, target, uploader)?;
    if !assisted(node, target, &acting)? {
        return Ok(AttachmentPlaces {
            machines: vec![node.machine().into()],
            selected: Some(node.machine().into()),
            images_only: false,
        });
    }
    owned(node, from, target, &acting, preferred)
}

fn owned(
    node: &Node,
    from: &MachineName,
    target: &UploadTarget,
    acting: &Acting,
    preferred: Option<&Written>,
) -> Result<AttachmentPlaces, Failure> {
    let Acting::Accounts { accounts } = acting else {
        return Err(Failure::refused("the assisted upload has no account"));
    };
    let mut machines = Vec::new();
    for identity in accounts {
        let link = node.link_of(&identity.hub).ok_or_else(|| failure("the upload account's hub is not connected"))?;
        let view = link.view();
        if view.account(&identity.account).is_none_or(|account| account.locked) {
            return Err(Failure::refused("the upload account is absent or locked"));
        }
        for member in view.machines().filter(|member| view.holds(&identity.account, &member.name) && member.online) {
            if view.admits(from, &identity.account, &member.name, Purpose::Hands).is_err() {
                continue;
            }
            machines.push(Written { name: member.name.clone(), hub: Some(link.hub_name()) });
        }
    }
    let matches = |wanted: &Written, candidate: &&Written| {
        wanted.name == candidate.name && wanted.hub.as_ref().is_none_or(|hub| Some(hub) == candidate.hub.as_ref())
    };
    let selected = if let Some(preferred) = preferred {
        Some(
            machines
                .iter()
                .find(|candidate| matches(preferred, candidate))
                .cloned()
                .ok_or_else(|| Failure::refused("choose one of this account's online machines for files"))?,
        )
    } else {
        let focus = match target.conversation_target() {
            UploadTarget::Resident { resident } => node.residents.about(resident).map(|(about, _)| about.focus),
            _ => None,
        };
        let focused: Vec<_> = machines
            .iter()
            .filter(|machine| {
                focus
                    .as_ref()
                    .is_some_and(|focus| focus.machines.iter().any(|wanted| matches(&wanted.machine, machine)))
            })
            .cloned()
            .collect();
        machines
            .iter()
            .find(|machine| machine.name == *from)
            .cloned()
            .or_else(|| (focused.len() == 1).then(|| focused[0].clone()))
            .or_else(|| (machines.len() == 1).then(|| machines[0].clone()))
    };
    Ok(AttachmentPlaces { images_only: machines.is_empty(), machines, selected })
}

pub(super) fn validate_original(node: &Node, acting: &Acting, original: &Attachment) -> Result<(), Failure> {
    let Acting::Accounts { accounts } = acting else { return Err(Failure::refused("the file has no account")) };
    for identity in accounts {
        let Some(link) = node.link_of(&identity.hub) else { continue };
        let view = link.view();
        if view.holds(&identity.account, &original.owner.machine)
            && view.admits(&view.me, &identity.account, &original.owner.machine, Purpose::Hands).is_ok()
        {
            return Ok(());
        }
    }
    Err(Failure::refused("the attachment original is not on this account's usable machine"))
}
