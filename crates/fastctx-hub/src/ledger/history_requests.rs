//! History reads use the same project authority and signed ledger as other records.

use crate::Hub;
use fastctx_net::wire::{Question, Told};
use fastctx_types::{AccountName, access::TownId};
use std::sync::Arc;

pub(crate) async fn ask(hub: &Arc<Hub>, from: &crate::origin::Origin, question: Question) -> Result<Told, String> {
    if let Question::HistorySubmitted { town, account, name, submission } = &question {
        let store = hub.store();
        super::requests::authorize(&store.book.directory, from, account, town, false)?;
        let ledgers = hub.ledgers.lock().expect("never poisoned");
        let mut revision =
            ledgers.towns.get(town).and_then(|ledger| ledger.history.submitted(name, submission)).cloned();
        if let Some(revision) = &mut revision {
            crate::history_projection::revision(hub, &store.book.directory, account, revision);
        }
        return Ok(Told::HistorySubmitted { revision: revision.map(Box::new) });
    }
    let (town, account, document) = match question {
        Question::HistoryCatalog { town, account } => (town, account, None),
        Question::HistoryRead { town, account, name, version, versions } => {
            (town, account, Some((name, version, versions)))
        }
        _ => unreachable!("history read"),
    };
    let check = |hub: &Hub| authorize(hub, from, &account, &town, document.is_some());
    let (path, versions, identity) = {
        let store = hub.store();
        super::requests::authorize(&store.book.directory, from, &account, &town, document.is_some())?;
        let ledgers = hub.ledgers.lock().expect("never poisoned");
        let ledger = ledgers.towns.get(&town).ok_or("the project has no ledger")?;
        let Some((name, version, include_versions)) = &document else {
            let mut catalog = ledger.history.catalog();
            for revision in &mut catalog.documents {
                crate::history_projection::revision(hub, &store.book.directory, &account, revision);
            }
            return Ok(Told::HistoryCatalog { catalog });
        };
        fastctx_types::history::markdown::check_name(name)?;
        let revision = ledger.history.get(name, *version).ok_or("the history document or revision does not exist")?;
        let versions = if *include_versions { ledger.history.versions(name) } else { Vec::new() };
        let seq = ledger
            .sequences
            .get(&fastctx_net::topic_history::path(name, revision.version, "md"))
            .ok_or("the history revision has no ledger entry")?;
        (ledgers.dir.join(town.as_str()).join(format!("{seq:020}.json")), versions, store.hub.id.clone())
    };
    let project = town.clone();
    let entry = fastctx_platform::task::spawn_blocking(move || {
        let signed = super::reading::entry(&path).map_err(|error| error.to_string())?;
        let entry = fastctx_net::ledger::open(&signed, &identity, &project)?;
        let stored = fastctx_net::topic_history::decode(&entry.files)?.ok_or("the entry has no history revision")?;
        let name = &stored.revision.name;
        let text = entry
            .files
            .iter()
            .find(|file| file.path == fastctx_net::topic_history::path(name, stored.revision.version, "md"))
            .ok_or("the history revision has no text")?;
        let text = String::from_utf8(text.bytes.0.clone()).map_err(|error| error.to_string())?;
        Ok::<_, String>(fastctx_types::history::Document { revision: stored.revision, text, versions })
    })
    .await
    .map_err(|error| error.to_string())??;
    check(hub)?;
    let mut document = entry;
    let store = hub.store();
    crate::history_projection::revision(hub, &store.book.directory, &account, &mut document.revision);
    for revision in &mut document.versions {
        crate::history_projection::revision(hub, &store.book.directory, &account, revision);
    }
    Ok(Told::HistoryDocument { document: Box::new(document) })
}

fn authorize(
    hub: &Hub,
    from: &crate::origin::Origin,
    account: &AccountName,
    town: &TownId,
    body: bool,
) -> Result<(), String> {
    super::requests::authorize(&hub.store().book.directory, from, account, town, body)
}
