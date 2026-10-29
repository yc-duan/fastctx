//! Project records go directly to the hub, including on a device with no online computer.

use super::{Device, failed};
use crate::Error;
use fastctx_net::{ledger::File, link::Link, sign::Bytes, wire::Change};
use fastctx_types::{
    AccountName, HubName, ProjectCall, Reply, TownName,
    access::TownId,
    memory::{Author, MemoryCall, MemoryReply},
    project::{MapView, Record, RecordFields, Stamp, TownSummary, Writer},
};
use std::sync::Arc;

type Scope = (Arc<Link>, AccountName, TownId);

impl Device {
    fn project_scope(&self, town: &TownName) -> Result<Scope, Error> {
        let mut scopes = Vec::new();
        for session in self.sessions() {
            let view = session.link.view();
            let Some(me) = view.me() else { continue };
            for project in view.projects.iter().filter(|project| project.name == *town) {
                let account = me
                    .logins
                    .iter()
                    .find(|account| **account == project.owner)
                    .or_else(|| me.logins.iter().find(|account| view.in_project(account, &project.town)));
                if let Some(account) = account {
                    scopes.push((session.link.clone(), account.clone(), project.town.clone()));
                }
            }
        }
        match scopes.len() {
            1 => Ok(scopes.pop().expect("one project")),
            0 => Err(failed(format!("no signed-in account has the project {town}"))),
            _ => Err(failed(format!("more than one hub has a project named {town}"))),
        }
    }

    pub(super) async fn memory(&self, request: MemoryCall) -> Result<MemoryReply, Error> {
        let (link, account, town) = self.project_scope(request.town())?;
        match request {
            MemoryCall::List { .. } => {
                Ok(MemoryReply::Catalog { catalog: link.project_index(&account, &town).await.map_err(failed)?.catalog })
            }
            MemoryCall::Read { name, version, .. } => Ok(MemoryReply::Document {
                document: Box::new(link.memory_read(&account, &town, &name, version).await.map_err(failed)?),
            }),
            MemoryCall::Save { kind, name, text, expected, .. } => {
                let author = Author {
                    stamp: Stamp { by: Writer::User, on: link.membership().name.clone(), at: jiff::Timestamp::now() },
                    host: None,
                    model: None,
                    session: None,
                };
                let (revision, files) =
                    fastctx_net::memory::revision_files(kind, &name, &text, expected, author).map_err(failed)?;
                link.memory_write(&account, &town, link.sign_entry(&link.entry(&town, None, files)))
                    .await
                    .map_err(failed)?;
                Ok(MemoryReply::Saved { revision: Box::new(revision) })
            }
        }
    }

    pub(super) async fn project(&self, request: ProjectCall) -> Result<Reply, Error> {
        if matches!(&request, ProjectCall::New { fields, .. } | ProjectCall::Edit { fields, .. } if fields.canon.is_some())
        {
            return Err(failed(fastctx_types::memory::LEGACY));
        }
        let map = matches!(&request, ProjectCall::Map { .. });
        match request {
            ProjectCall::List => {
                let mut towns = Vec::new();
                for session in self.sessions() {
                    let view = session.link.view();
                    let Some(me) = view.me() else { continue };
                    let selected: Vec<_> = view
                        .projects
                        .iter()
                        .filter_map(|project| {
                            let account = me.logins.iter().find(|account| view.in_project(account, &project.town))?;
                            Some((project.clone(), account.clone()))
                        })
                        .collect();
                    drop(view);
                    for (project, account) in selected {
                        let index = session.link.project_index(&account, &project.town).await.map_err(failed)?;
                        let mut record = index.record;
                        visible(&session.link, &account, &mut record);
                        towns.push(TownSummary {
                            town: project.name.clone(),
                            record,
                            journals: index.catalog.journals.len(),
                        });
                    }
                }
                towns.sort_by(|a, b| a.town.cmp(&b.town));
                Ok(Reply::Towns { towns })
            }
            ProjectCall::Show { town } | ProjectCall::Map { town } => {
                let town = town.ok_or_else(|| failed("name the project"))?;
                let (link, account, id) = self.project_scope(&town)?;
                let index = link.project_index(&account, &id).await.map_err(failed)?;
                let mut record = index.record;
                visible(&link, &account, &mut record);
                if map {
                    Ok(Reply::Map {
                        map: Box::new(MapView {
                            memory: None,
                            town,
                            ledger: String::new(),
                            record,
                            opened: index.opened,
                            journals: Vec::new(),
                            pages: index.pages,
                            canon: None,
                        }),
                    })
                } else {
                    Ok(Reply::Record { town, record: Box::new(record), opened: index.opened })
                }
            }
            ProjectCall::Edit { town, fields } => {
                if fields == RecordFields::default() {
                    return Err(failed("nothing to change in the project record"));
                }
                let town = town.ok_or_else(|| failed("name the project"))?;
                let (link, account, id) = self.project_scope(&town)?;
                let index = link.project_index(&account, &id).await.map_err(failed)?;
                let mut record = index.record;
                record.version = record.version.checked_add(1).ok_or_else(|| failed("project version exhausted"))?;
                change(&mut record, fields);
                record.by = Writer::User;
                record.on = link.membership().name.clone();
                record.at = jiff::Timestamp::now();
                checked(&link, &account, &mut record)?;
                publish(&link, &id, &record).await?;
                visible(&link, &account, &mut record);
                Ok(Reply::Record { town, record: Box::new(record), opened: index.opened })
            }
            ProjectCall::New { town, fields, account, hub } => {
                let (link, account) = self.project_account(hub.as_ref(), account.as_ref())?;
                if !link.view().project_memory {
                    return Err(failed("update the hub to create projects directly from this device"));
                }
                let machines = fields.machines.clone().unwrap_or_else(|| {
                    let view = link.view();
                    view.machines()
                        .filter(|member| view.holds(&account, &member.name) && !member.managed)
                        .map(|member| member.name.clone())
                        .collect()
                });
                let mut record = Record {
                    version: 1,
                    name: fields.name.clone().ok_or_else(|| failed("a project needs a name"))?,
                    what: fields.what.clone().ok_or_else(|| failed("a project needs its instructions"))?,
                    machines,
                    places: Vec::new(),
                    canon: None,
                    by: Writer::User,
                    on: link.membership().name.clone(),
                    at: jiff::Timestamp::now(),
                };
                change(&mut record, fields);
                checked(&link, &account, &mut record)?;
                let id = TownId::from_bytes(fastctx_net::sign::random::<8>().map_err(failed)?);
                link.manage(&account, Change::OpenProject { town: id.clone(), name: town.clone() })
                    .await
                    .map_err(failed)?;
                publish(&link, &id, &record).await?;
                let opened = Stamp { by: record.by.clone(), on: record.on.clone(), at: record.at };
                Ok(Reply::Record { town, record: Box::new(record), opened })
            }
            ProjectCall::Enter { .. } => Err(failed("a device is not a conversation and cannot enter a project")),
        }
    }

    pub(super) fn project_account(
        &self,
        hub: Option<&HubName>,
        account: Option<&AccountName>,
    ) -> Result<(Arc<Link>, AccountName), Error> {
        let mut choices = Vec::new();
        for session in
            self.sessions().into_iter().filter(|session| hub.is_none_or(|hub| session.link.hub_name() == *hub))
        {
            if let Some(me) = session.link.view().me() {
                for name in me.logins.iter().filter(|name| account.is_none_or(|account| account == *name)) {
                    choices.push((session.link.clone(), name.clone()));
                }
            }
        }
        match choices.len() {
            1 => Ok(choices.pop().expect("one account")),
            0 => Err(failed("no matching signed-in account")),
            _ => Err(failed("choose a hub and account")),
        }
    }
}

fn change(record: &mut Record, fields: RecordFields) {
    if let Some(name) = fields.name {
        record.name = name;
    }
    if let Some(what) = fields.what {
        record.what = what;
    }
    if let Some(machines) = fields.machines {
        record.machines = machines;
    }
    if let Some(places) = fields.places {
        record.places = places;
    }
    if let Some(canon) = fields.canon {
        record.canon = Some(canon);
    }
}
fn checked(link: &Link, account: &AccountName, record: &mut Record) -> Result<(), Error> {
    if record.machines.is_empty() {
        return Err(failed("a project spans at least one machine"));
    }
    let view = link.view();
    for machine in record
        .machines
        .iter_mut()
        .chain(record.places.iter_mut().map(|place| &mut place.location.machine))
        .chain(record.canon.iter_mut().map(|location| &mut location.machine))
    {
        if !view.member(machine).is_some_and(|member| member.kind == fastctx_types::accounts::MemberKind::Computer)
            || !view.rights(account, machine).hands
        {
            return Err(failed(format!("the account may not work on {machine}")));
        }
        *machine = view.canonical(machine).clone();
    }
    for place in &mut record.places {
        place.location.path = absolute(&place.location.path)?;
        if place.what.trim().is_empty() {
            return Err(failed("a place needs a description"));
        }
    }
    if let Some(canon) = &mut record.canon {
        canon.path = absolute(&canon.path)?;
    }
    Ok(())
}
fn absolute(path: &str) -> Result<String, Error> {
    let bytes = path.as_bytes();
    let drive =
        bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && matches!(bytes[2], b'/' | b'\\');
    let unc = path.starts_with("\\\\") || path.starts_with("//");
    if path.starts_with('/') && !unc {
        Ok(path.into())
    } else if drive || unc {
        Ok(path.replace('\\', "/"))
    } else {
        Err(failed("a project place needs an absolute path"))
    }
}
async fn publish(link: &Link, town: &TownId, record: &Record) -> Result<(), Error> {
    let files = vec![File {
        path: fastctx_types::project::record_path(record.version),
        bytes: Bytes(serde_json::to_vec(record).map_err(failed)?),
    }];
    link.ledger_write(town, link.sign_entry(&link.entry(town, Some(record.machines.clone()), files)))
        .await
        .map_err(failed)?;
    Ok(())
}
fn visible(link: &Link, account: &AccountName, record: &mut Record) {
    let view = link.view();
    let usable = |machine: &fastctx_types::MachineName| view.rights(account, view.canonical(machine)).hands;
    record.machines.retain(usable);
    record.places.retain(|place| usable(&place.location.machine));
    if record.canon.as_ref().is_some_and(|canon| !usable(&canon.machine)) {
        record.canon = None;
    }
}
