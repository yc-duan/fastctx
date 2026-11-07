//! Account-scoped metadata and document reads, independent of online computers.

use std::path::PathBuf;
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
        first: (String, PathBuf),
        last: (String, PathBuf),
        pages: Vec<(String, PathBuf)>,
    },
    Document {
        path: PathBuf,
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
                let path = |seq: u64| ledgers.dir.join(town.as_str()).join(format!("{seq:020}.json"));
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
            let project = town.clone();
            crate::readers::blocking(move || plan.read(&hub_id, &project))
                .await
                .map_err(|error| format!("reading project memory: {error}"))?
        })
        .await;
        let answer = authorize(&hub.store().book.directory, &from, &account, &town, body).and(answer);
        let _ = out.send(Out::Frame(Down::Told { id, answer }));
    });
}

impl Plan {
    fn read(self, hub: &fastctx_net::ids::HubId, town: &TownId) -> Result<Told, String> {
        let entry = |path: &PathBuf| {
            let signed = super::reading::entry(path).map_err(|error| error.to_string())?;
            let opened = fastctx_net::ledger::open(&signed, hub, town)?;
            Ok::<_, String>((signed, opened))
        };
        match self {
            Self::Index { seq, catalog, first, last, pages } => {
                let record = |(file, path): &(String, PathBuf)| {
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
                Ok(Told::ProjectIndex {
                    index: Box::new(ProjectIndex {
                        seq,
                        record: record(&last)?,
                        opened,
                        catalog,
                        pages: super::page_index::read(pages, hub, town)?,
                    }),
                })
            }
            Self::Document { path, versions, evidence } => {
                let (entry, _) = entry(&path)?;
                Ok(Told::MemoryDocument { entry, versions, evidence })
            }
        }
    }
}
