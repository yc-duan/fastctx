//! Durable project deletion work, delivered in each affected member's view.

use fastctx_net::directory::Project;
use fastctx_net::sign::PublicKey;
use fastctx_net::wire::Told;
use fastctx_types::MachineName;
use fastctx_types::access::TownId;
use serde::{Deserialize, Serialize};

use crate::{Hub, store::Store};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Pending {
    pub project: Project,
    waiting: Vec<PublicKey>,
}

pub(crate) fn begin(store: &mut Store, project: Project) -> Result<(), String> {
    let directory = &store.book.directory;
    for member in directory
        .members
        .iter()
        .filter(|member| member.is_machine() && directory.member_in_project(&member.name, &project.town))
    {
        if !store.letters.get(&member.name).is_some_and(|said| said.conversation_deletions) {
            return Err(format!("update {} before deleting a project with conversations there", member.name));
        }
    }
    let waiting = directory
        .members
        .iter()
        .filter(|member| member.is_machine() && directory.member_in_project(&member.name, &project.town))
        .map(|member| member.key)
        .collect();
    store.book.project_deletions.push(std::sync::Arc::new(Pending { project, waiting }));
    Ok(())
}

pub(crate) fn for_member(store: &Store, name: &MachineName) -> Vec<Project> {
    let Some(member) = store.book.directory.member(name) else { return Vec::new() };
    store
        .book
        .project_deletions
        .iter()
        .filter(|pending| pending.waiting.contains(&member.key))
        .map(|pending| pending.project.clone())
        .collect()
}

pub(crate) fn finished(hub: &Hub, member: &MachineName, town: &TownId) -> Result<Told, String> {
    {
        let mut store = hub.store();
        let key =
            store.book.directory.member(member).ok_or_else(|| "the member is no longer on the hub".to_string())?.key;
        if let Some(pending) = store.book.project_deletions.iter_mut().find(|pending| pending.project.town == *town) {
            std::sync::Arc::make_mut(pending).waiting.retain(|waiting| *waiting != key);
        }
        store.book.project_deletions.retain(|pending| !pending.waiting.is_empty());
        store.save_book().map_err(|error| error.to_string())?;
    }
    hub.changed();
    Ok(Told::Done)
}
