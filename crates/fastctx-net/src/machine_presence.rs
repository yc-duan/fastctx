//! Visibility is computed from each link's directory, never from a missing reply.

use std::collections::{BTreeSet, HashSet};
use std::sync::Arc;

use fastctx_types::access::{Purpose, Written};
use fastctx_types::town::{MachineWho, WorldWho};
use fastctx_types::world::{HubMachinePresence, MachineAccess, MachinePresence};

use crate::directory::View;
use crate::directory_index::{Index, Indexed};
use crate::link::{Link, Status};

struct Directory {
    view: Arc<View>,
    index: Index,
    stopped: bool,
    current: bool,
}

impl Directory {
    fn indexed(&self) -> Indexed<'_> {
        Indexed::new(&self.view.directory, &self.index)
    }
}

/// Decides Look with the same source accounts and purpose used to list conversations.
/// Only a directory on this connection may establish that permission is no longer granted.
pub fn of(links: &[Arc<Link>], who: &WorldWho, known: &[String]) -> Vec<MachinePresence> {
    let directories: Vec<_> = links
        .iter()
        .map(|link| {
            let view = link.view();
            let status = link.status();
            // A refresh can replace the view before marking this connection current.
            let current =
                matches!(status, Status::Online { .. }) && link.view_current() && Arc::ptr_eq(&view, &link.view());
            let index = Index::new(&view.directory);
            Directory { view, index, stopped: matches!(status, Status::Stopped { .. }), current }
        })
        .collect();
    let mut names: BTreeSet<String> = known.iter().cloned().collect();
    names.extend(who.machines.iter().map(|machine| match machine {
        MachineWho::Answered { reply, .. } => reply.machine.to_string(),
        MachineWho::Offline { machine, .. } | MachineWho::NoAnswer { machine, .. } => machine.to_string(),
    }));
    names
        .into_iter()
        .map(|machine| {
            let written = machine.parse::<Written>().ok();
            let keys: HashSet<_> = directories
                .iter()
                .filter_map(|directory| {
                    let name = written.as_ref()?;
                    if name.hub.as_ref().is_some_and(|hub| *hub != directory.view.hub_name) {
                        return None;
                    }
                    directory.indexed().member(&name.name).map(|member| member.key)
                })
                .collect();
            let hubs = directories
                .iter()
                .filter(|directory| {
                    written
                        .as_ref()
                        .is_some_and(|name| name.hub.as_ref().is_none_or(|hub| *hub == directory.view.hub_name))
                })
                .map(|directory| {
                    let view = &directory.view;
                    let name = &written.as_ref().expect("a parsed name selected this directory").name;
                    let index = directory.indexed();
                    let accounts = index.member(&view.me).into_iter().flat_map(|me| me.logins.iter());
                    let member = if keys.len() == 1 {
                        keys.iter().find_map(|key| index.by_key(key))
                    } else {
                        index.member(name)
                    };
                    let listed = member.is_some_and(|member| {
                        member.is_machine()
                            && index.account_for(accounts, &view.me, &member.name, Purpose::Look).is_some()
                    });
                    let access = if directory.stopped {
                        MachineAccess::Stopped
                    } else if keys.len() > 1 {
                        // An unqualified name cannot identify different devices at once.
                        MachineAccess::Unknown
                    } else if listed {
                        MachineAccess::Granted
                    } else if directory.current && view.me().is_some() {
                        MachineAccess::NotGranted
                    } else {
                        MachineAccess::Unknown
                    };
                    HubMachinePresence {
                        hub: view.hub.clone(),
                        revision: view.revision,
                        current: directory.current,
                        access,
                    }
                })
                .collect();
            MachinePresence { machine, hubs }
        })
        .collect()
}
