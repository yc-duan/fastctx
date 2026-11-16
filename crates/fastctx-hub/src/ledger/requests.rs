//! Account-scoped metadata and document reads, independent of online computers.


use std::sync::Arc;
use std::time::Duration;

use fastctx_net::authorization::Action;
use fastctx_net::directory::Directory;

use fastctx_net::wire::{Down, Question, Told};
use fastctx_types::AccountName;
use fastctx_types::access::TownId;
use fastctx_types::memory::{Catalog, Evidence, ProjectIndex, Revision};
use fastctx_types::project::{Record, Stamp, version_of};

use crate::outq::Outq;
use crate::{Hub, Out};

pub(super) fn authorize(
    directory: &Directory,
    from: &crate::origin::Origin,
    account: &AccountName,
    town: &TownId,
    body: bool,
) -> Result<(), String> {
    from.check(directory)?;
    directory.authorize(account, Action::ActFor(from)).map_err(|denial| denial.message)?;
    directory.authorize(account, Action::Project(town)).map_err(|denial| denial.message)?;
    if body && !directory.logged_in(from, account) {
        return Err(fastctx_types::memory::words::BODY_DEVICE_REQUIRED.into());
    }
    Ok(())
}

enum Plan {
    Index {
        seq: u64,
        catalog: Catalog,
        first: (String, super::EntryAt),
        last: (String, super::EntryAt),
        pages: Vec<(String, super::EntryAt)>,
    },
    Document {
        path: super::EntryAt,
        versions: Vec<Revision>,
        evidence: Vec<Evidence>,
    },
}

pub(crate) fn ask(hub: &Arc<Hub>, from: &crate::origin::Origin, out: &Outq, id: u64, question: Question) {
    let (town, account, wanted) = match question {
        Question::ProjectIndex { town, account } => (town, account, None),
        Question::MemoryRead { town, account, name, version } => (town, account, Some((name, version))),
        _ => unreachable!("only project reads"),
    };
    let body = wanted.is_some();
    let (hub, from, out) = (hub.clone(), from.clone(), out.clone());
    let deadline = tokio::time::Instant::now() + Duration::from_secs(55);
    fastctx_platform::task::spawn(async move {
        let permit = match crate::readers::account(&hub, &from, &account, &out, deadline).await {
            Ok(permit) => permit,
            Err(why) => {
                let _ = out.send(Out::Frame(Down::Told { id, answer: Err(why) }));
                return;
            }
        };
        let answer = crate::readers::run(permit, &out, deadline, async {
            let prepare = || {
                let store = hub.store();
                authorize(&store.book.directory, &from, &account, &town, body)?;
                let ledgers = hub.ledgers.lock().expect("never poisoned");
                let ledger = ledgers.towns.get(&town).ok_or(fastctx_types::memory::words::NO_LEDGER)?;
                let path = |seq: u64| ledgers.entry_at(&town, seq);
                let plan = if let Some((name, version)) = &wanted {
                    let versions = ledger.memory.versions(name);
                    let revision = match version {
                        Some(version) => versions.iter().find(|revision| revision.version == *version),
                        None => versions.last(),
                    }
                    .ok_or(fastctx_types::memory::words::NO_DOCUMENT)?;
                    let seq = *ledger
                        .sequences
                        .get(&revision.path("md"))
                        .ok_or(fastctx_types::memory::words::MISSING_REVISION_ENTRY)?;
                    Plan::Document { path: path(seq), evidence: ledger.memory.evidence(&revision.name), versions }
                } else {
                    let mut records: Vec<_> = ledger
                        .sequences
                        .range("project/".to_owned()..)
                        .take_while(|(path, _)| path.starts_with("project/"))
                        .filter_map(|(file, seq)| {
                            let name = file.strip_prefix("project/")?;
                            Some((version_of(name)?, file.clone(), path(*seq)))
                        })
                        .collect();
                    records.sort_by_key(|(version, _, _)| *version);
                    let (_, first, first_path) =
                        records.first().ok_or(fastctx_types::memory::words::MISSING_OPENING)?;
                    let (_, last, last_path) = records.last().expect("has a first record");
                    Plan::Index {
                        seq: ledger.next - 1,
                        catalog: ledger.memory.catalog(),
                        first: (first.clone(), first_path.clone()),
                        last: (last.clone(), last_path.clone()),
                        pages: ledger
                            .sequences
                            .range("pages/".to_owned()..)
                            .take_while(|(path, _)| path.starts_with("pages/"))
                            .filter(|(file, _)| file.ends_with("/page.json"))
                            .map(|(file, seq)| (file.clone(), path(*seq)))
                            .collect(),
                    }
                };
                Ok::<_, String>((plan, store.hub.id.clone()))
            };
            let (plan, hub_id) = prepare()?;
            // Told as the asking machine's account may know it: the machines
            // it names only where that account may work, or this machine
            // (M09, §29 2.4).
            let usable = super::for_reader::usable(&hub.store().book.directory, &account, Some(&from.name));
            let (project, signing, asking) = (town.clone(), hub.assets.clone(), from.name.clone());
            crate::readers::blocking(move || {
                let usable = |machine: &fastctx_types::MachineName| usable.contains(machine);
                plan.read(&hub_id, &project, &usable, &|entry| signing.keys().sign_entry(entry), &asking)
            })
            .await
            .map_err(|error| format!("reading project memory: {error}"))?
        })
        .await;
        let answer = authorize(&hub.store().book.directory, &from, &account, &town, body).and(answer);
        let _ = out.send(Out::Frame(Down::Told { id, answer }));
    });
}

impl Plan {
    fn read(
        self,
        hub: &fastctx_net::ids::HubId,
        town: &TownId,
        usable: &dyn Fn(&fastctx_types::MachineName) -> bool,
        sign: &dyn Fn(fastctx_net::ledger::Entry) -> fastctx_net::sign::Signed,
        asking: &fastctx_types::MachineName,
    ) -> Result<Told, String> {
        let entry = |path: &super::EntryAt| {
            let signed = super::reading::entry(path).map_err(|error| error.to_string())?;
            let opened = fastctx_net::ledger::open(&signed, hub, town)?;
            Ok::<_, String>((signed, opened))
        };
        match self {
            Self::Index { seq, catalog, first, last, pages } => {
                let record = |(file, path): &(String, super::EntryAt)| {
                    let (_, entry) = entry(path)?;
                    let bytes = &entry
                        .files
                        .iter()
                        .find(|item| item.path == *file)
                        .ok_or(fastctx_types::memory::words::MISSING_RECORD)?
                        .bytes
                        .0;
                    serde_json::from_slice::<Record>(bytes).map_err(|error| error.to_string())
                };
                let first = record(&first)?;
                let opened = Stamp { by: first.by, on: first.on, at: first.at };
                let mut index = ProjectIndex {
                    seq,
                    record: record(&last)?,
                    opened,
                    catalog,
                    pages: super::page_index::read(pages, hub, town)?,
                };
                super::for_reader::index(&mut index, usable);
                Ok(Told::ProjectIndex { index: Box::new(index) })
            }
            Self::Document { path, mut versions, mut evidence } => {
                let (signed, mut opened) = entry(&path)?;
                super::for_reader::document(&mut versions, &mut evidence, usable);
                // The author's own signature while nothing in it is told
                // otherwise; else the hub's, the entry named as the asking
                // machine's, which a reader checks the same way.
                // Its envelope too: the machine that wrote it, and those it
                // names, only where the reader may work.
                let files_changed = super::for_reader::filter(&mut opened.files, usable, &|_| {});
                let envelope_hides =
                    !usable(&opened.author) || opened.machines.as_ref().is_some_and(|machines| machines.iter().any(|machine| !usable(machine)));
                let entry = if files_changed || envelope_hides {
                    opened.author = asking.clone();
                    opened.machines = None;
                    sign(opened)
                } else {
                    signed
                };
                Ok(Told::MemoryDocument { entry, versions, evidence })
            }
        }
    }
}
