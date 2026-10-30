//! Readers before reply shapes 1 (`ipc::SHAPES`) cannot read a stamp told
//! without the machine it was written on, nor a writer told only as a
//! conversation or a page elsewhere: what whoever may not work on that
//! machine is told (`project::Writer::for_reader`).

use fastctx_types::Reply;
use fastctx_types::memory::{Author, MemoryReply};
use fastctx_types::pages::PageVersion;
use fastctx_types::project::{JournalSummary, MapView, Record, Stamp, TownSummary, Writer};
use fastctx_types::since::SinceKind;
use fastctx_types::town::WhoScope;

fn writer(by: &Writer) -> bool {
    matches!(by, Writer::Elsewhere { .. })
}

fn stamp(stamp: &Stamp) -> bool {
    stamp.on.is_none() || writer(&stamp.by)
}

fn record(record: &Record) -> bool {
    record.on.is_none() || writer(&record.by)
}

fn version(version: &PageVersion) -> bool {
    version.on.is_none() || writer(&version.by)
}

fn author(author: &Author) -> bool {
    stamp(&author.stamp)
}

fn towns(towns: &[TownSummary]) -> bool {
    towns.iter().any(|town| record(&town.record))
}

fn scope(scope: &WhoScope) -> bool {
    match scope {
        WhoScope::Town { record: told, .. } => told.as_deref().is_some_and(record),
        WhoScope::World { towns: told } => towns(told),
    }
}

fn journal(journal: &JournalSummary) -> bool {
    journal.description.as_ref().is_some_and(|description| writer(&description.by))
        || journal.latest.as_ref().is_some_and(|entry| writer(&entry.by))
}

fn map(map: &MapView) -> bool {
    record(&map.record)
        || stamp(&map.opened)
        || map.pages.iter().any(|page| version(&page.current))
        || map.journals.iter().any(journal)
}

/// What an interface keeps open, without the projects it could not read:
/// one whose record or opening is told without its machine is left out,
/// rather than the whole of it failing to be read.
pub(super) fn leave_out(world: &mut fastctx_types::world::WorldNow) {
    world.towns.retain(|town| !record(&town.record));
    if let WhoScope::World { towns } = &mut world.who.scope {
        towns.retain(|town| !record(&town.record));
    }
    if let WhoScope::Town { record: told, .. } = &mut world.who.scope
        && told.as_deref().is_some_and(record)
    {
        *told = None;
    }
}

pub(super) fn present(reply: &Reply) -> bool {
    match reply {
        Reply::Who(who) => scope(&who.scope),
        Reply::WorldNow(now) => scope(&now.who.scope) || towns(&now.towns),
        Reply::Towns { towns: told } => towns(told),
        Reply::Record { record: told, opened, .. } => record(told) || stamp(opened),
        Reply::Map { map: view } | Reply::Entered { map: view, .. } => map(view),
        Reply::Page { view } => {
            version(&view.publication) || view.consent.as_ref().is_some_and(|consent| stamp(&consent.stamp))
        }
        Reply::Pages { pages, .. } => pages.iter().any(|page| version(&page.current)),
        Reply::PageHistory { versions, .. } => versions.iter().any(version),
        Reply::Memory(MemoryReply::Catalog { catalog }) => {
            catalog.journals.iter().chain(&catalog.specs).any(|revision| author(&revision.author))
        }
        Reply::Memory(MemoryReply::Document { document }) => {
            author(&document.revision.author)
                || document.versions.iter().any(|revision| author(&revision.author))
                || document.evidence.iter().any(|evidence| author(&evidence.author))
        }
        Reply::Memory(MemoryReply::Saved { revision }) => author(&revision.author),
        Reply::Journals { journals, .. } => journals.iter().any(journal),
        Reply::Journal { view } => {
            view.description.as_ref().is_some_and(|description| writer(&description.by))
                || view.entries.iter().any(|entry| writer(&entry.stamp.by))
        }
        Reply::Since(since) => since.events.iter().any(|event| match &event.what {
            SinceKind::ProjectVersion { by, .. }
            | SinceKind::JournalEntry { by, .. }
            | SinceKind::JournalDescription { by, .. } => writer(by),
            _ => false,
        }),
        _ => false,
    }
}
