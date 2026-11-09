//! Verify the carried prefix and retire standalone ledger files before changing identity.

use super::*;
use crate::history::import::archive::{self, Original};
use fastctx_types::{AccountName, history::Scope};
use serde::{Deserialize, Serialize};
#[path = "carry_history.rs"]
mod history;
#[path = "carry_retained.rs"]
mod retained;
#[path = "carry_topics.rs"]
mod topics;
use retained::Retained;

#[derive(Serialize, Deserialize)]
struct Journal {
    source: Place,
    target: Place,
    account: AccountName,
    opened: jiff::Timestamp,
    #[serde(default)]
    originals: Option<Vec<Original>>,
    #[serde(default)]
    retained: Vec<Retained>,
    #[serde(default)]
    skipped: BTreeMap<String, String>,
    #[serde(default)]
    classified: bool,
    #[serde(default)]
    reported: bool,
    #[serde(default)]
    archived: bool,
    #[serde(default)]
    finished: bool,
}

fn path(world: &WorldDir, town: &TownName) -> PathBuf {
    world.town(town).path().join("ledger-import.json")
}

pub(crate) struct Target {
    pub place: Place,
    pub account: AccountName,
    pub opened: jiff::Timestamp,
}

pub(crate) fn target(world: &WorldDir, town: &TownName) -> io::Result<Option<Target>> {
    Ok(fastctx_net::link::state::read_or::<Option<Journal>>(&path(world, town), None)?
        .filter(|journal| !journal.finished)
        .map(|journal| Target { place: journal.target, account: journal.account, opened: journal.opened }))
}

pub(super) fn writing(world: &WorldDir, town: &TownName) -> io::Result<bool> {
    Ok(fastctx_net::link::state::read_or::<Option<Journal>>(&path(world, town), None)?
        .is_some_and(|journal| !journal.finished))
}

pub(crate) fn pending(world: &WorldDir, town: &TownName) -> bool {
    let journal = fastctx_net::link::state::read_or::<Option<Journal>>(&path(world, town), None);
    if !matches!(&journal, Ok(None) | Ok(Some(Journal { finished: true, .. }))) {
        return true;
    }
    match names::place(world, town) {
        Ok(Some(place)) => alone(world).map_or(true, |id| place.hub == id),
        Ok(None) => false,
        Err(_) => true,
    }
}

pub(super) struct Originals {
    pub place: Place,
    pub archived: Vec<Original>,
    pub retained: Vec<Retained>,
}

pub(super) fn originals(world: &WorldDir, town: &TownName) -> io::Result<Option<Originals>> {
    Ok(fastctx_net::link::state::read_or::<Option<Journal>>(&path(world, town), None)?
        .filter(|journal| !journal.finished)
        .and_then(|journal| {
            journal.originals.map(|archived| Originals { place: journal.source, archived, retained: journal.retained })
        }))
}

/// Startup completes only a handoff whose remote prefix was already verified.
pub(crate) fn recover(world: &WorldDir, town: &TownName, history_enabled: bool) -> io::Result<()> {
    if let Some(mut journal) = fastctx_net::link::state::read_or::<Option<Journal>>(&path(world, town), None)?
        && !journal.finished
        && journal.originals.is_some()
        && (history_enabled || !has_history_originals(world, town, &journal))
    {
        finish(world, town, &mut journal)?;
    }
    Ok(())
}

fn has_history_originals(world: &WorldDir, town: &TownName, journal: &Journal) -> bool {
    let history = world.town(town).ledger().join("history");
    !journal.skipped.is_empty()
        || journal.originals.iter().flatten().any(|original| original.source().starts_with(&history))
        || journal.retained.iter().any(|retained| retained.source().starts_with(&history))
}

fn finish(world: &WorldDir, town: &TownName, journal: &mut Journal) -> io::Result<()> {
    if !journal.archived {
        for retained in &journal.retained {
            retained.finish()?;
        }
        for original in journal.originals.as_ref().expect("verified handoff") {
            original.finish()?;
        }
        let directory = world.town(town);
        archive::empty(&directory.signed())?;
        archive::empty(&directory.ledger())?;
        // The new identity reuses project.json; its old source must never be archived again.
        journal.archived = true;
        fastctx_net::link::state::write(&path(world, town), journal)?;
    }
    names::set_place(world, town, &journal.target)?;
    journal.finished = true;
    journal.originals = None;
    journal.retained.clear();
    fastctx_net::link::state::write(&path(world, town), journal)
}

pub(crate) async fn run(node: &Arc<Node>, hub: &HubId) -> Vec<String> {
    let Some(link) = node.link_of(hub) else { return Vec::new() };
    if let Err(error) = link.refresh().await {
        return vec![format!("asking the hub: {error}")];
    }
    let Some(home) = link.view().me().and_then(|me| me.home.clone()) else { return Vec::new() };
    let towns = match node.world.towns() {
        Ok(towns) => towns,
        Err(error) => return vec![error.to_string()],
    };
    let mut problems = Vec::new();
    for town in towns {
        let scope = Scope::Project { project: town.clone() };
        crate::history::import::pending(node, &scope);
        if let Err(error) = one(node, &link, &home, &town).await.and_then(|()| report(node, &town)) {
            problems.push(format!("carrying {town}: {error}"));
        }
        crate::history::import::pending(node, &scope);
    }
    problems
}

async fn one(node: &Arc<Node>, link: &Arc<Link>, home: &AccountName, town: &TownName) -> Result<(), String> {
    let scope = Scope::Project { project: town.clone() };
    let (mut journal, entries) = {
        let _history = crate::history::import::lock(node, &scope).await;
        let _syncing = node.syncing.lock().await;
        let prior: Option<Journal> =
            fastctx_net::link::state::read_or(&path(&node.world, town), None).map_err(|error| error.to_string())?;
        let history_enabled = crate::history::switch::on(link, home);
        if let Some(journal) = prior.as_ref().filter(|journal| !journal.finished) {
            if journal.target.hub != link.membership().id
                || journal.account != *home
                || link.view().account(home).is_none_or(|account| account.opened != journal.opened)
            {
                return Err("the standalone ledger is being imported into another account or hub incarnation".into());
            }
            if !history_enabled && has_history_originals(&node.world, town, journal) {
                return Ok(());
            }
        }
        let recovering = originals(&node.world, town).map_err(|error| error.to_string())?.is_some();
        recover(&node.world, town, history_enabled).map_err(|error| error.to_string())?;
        if recovering {
            crate::context::invalidations::project(node, town);
            crate::replicas::updates::schedule(node, link.hub(), crate::replicas::updates::Change::All);
            node.memory_changed();
        }
        let Some(source) = names::place(&node.world, town).map_err(|error| error.to_string())? else { return Ok(()) };
        if source.hub != alone(&node.world).map_err(|error| error.to_string())? {
            return Ok(());
        }
        let entries = read_all(&node.world, &node.world.town(town)).map_err(|error| error.to_string())?;
        if !history_enabled && entries.iter().any(|(_, entry, _)| fastctx_net::topic_history::has_history(&entry.files))
        {
            return Ok(());
        }
        if entries.is_empty() {
            return Ok(());
        }
        let opened = link.view().account(home).ok_or("the importing account is unavailable")?.opened;
        let journal = fastctx_net::link::state::read_or(&path(&node.world, town), None)
            .map_err(|error| error.to_string())?
            .unwrap_or(Journal {
                source: source.clone(),
                target: Place { hub: link.membership().id, town: source.town.clone() },
                account: home.clone(),
                opened,
                originals: None,
                retained: Vec::new(),
                skipped: BTreeMap::new(),
                classified: false,
                reported: false,
                archived: false,
                finished: false,
            });
        if journal.source != source
            || journal.target.hub != link.membership().id
            || journal.account != *home
            || journal.opened != opened
        {
            return Err("the standalone ledger is being imported into another account or hub incarnation".into());
        }
        fastctx_net::link::state::write(&path(&node.world, town), &journal).map_err(|error| error.to_string())?;
        (journal, entries)
    };
    let has_history = entries.iter().any(|(_, entry, _)| fastctx_net::topic_history::has_history(&entry.files));
    let enabled = || !has_history || crate::history::switch::on(link, home);
    if !enabled() {
        return Ok(());
    }
    let own = own_key(&node.world).map_err(|error| error.to_string())?.public();
    let open = fastctx_net::wire::Change::OpenProject {
        town: journal.target.town.clone(),
        name: town.clone(),
        shared_on: link.view().shared_records.then(|| node.machine()),
    };
    if let Err(error) = link.manage(home, open).await
        && link.view().project(&journal.target.town).is_none_or(|project| project.name != *town)
    {
        return Err(error);
    }
    let id = &journal.target.town;
    if !journal.classified {
        journal.skipped = topics::skipped(link, home, id, &entries).await?;
        journal.classified = true;
        fastctx_net::link::state::write(&path(&node.world, town), &journal).map_err(|error| error.to_string())?;
    }
    let selected = entries
        .iter()
        .enumerate()
        .filter_map(|(index, (_, entry, _))| match topics::omitted(entry, &journal.skipped) {
            Ok(true) => None,
            Ok(false) => Some(Ok(index)),
            Err(error) => Some(Err(error)),
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut written = prefix(node, link, id, &entries, &selected, &own).await?;
    while written < selected.len() {
        if !enabled() {
            return Ok(());
        }
        let entry = translated(node, link, &entries[selected[written]].1, &own)?;
        let signed = link.sign_entry(&entry);
        let result = if fastctx_net::topic_history::has_history(&entry.files) {
            link.history_write(home, id, signed).await
        } else if fastctx_net::memory::has_memory(&entry.files) {
            link.memory_write(home, id, signed).await
        } else {
            link.ledger_write(id, signed).await
        };
        match result {
            Ok(seq) if seq == written as u64 + 1 => written += 1,
            result => {
                let checked = prefix(node, link, id, &entries, &selected, &own).await?;
                if checked <= written {
                    return Err(result.err().unwrap_or_else(|| "the hub inserted an unrelated ledger entry".into()));
                }
                written = checked;
            }
        }
    }
    // Verify every entry, including writes whose acknowledgment arrived normally.
    if prefix(node, link, id, &entries, &selected, &own).await? != selected.len() {
        return Err("the carried ledger is incomplete".into());
    }
    let _history = crate::history::import::lock(node, &scope).await;
    let _syncing = node.syncing.lock().await;
    if !enabled() {
        return Ok(());
    }
    if names::place(&node.world, town).map_err(|error| error.to_string())?.as_ref() != Some(&journal.source)
        || link.view().account(home).is_none_or(|account| account.opened != journal.opened)
        || link.view().me().and_then(|me| me.home.as_ref()) != Some(home)
    {
        return Err("the project or importing account changed during the handoff".into());
    }
    let current = read_all(&node.world, &node.world.town(town)).map_err(|error| error.to_string())?;
    if current.len() != entries.len() || current.iter().zip(&entries).any(|(a, b)| a.0 != b.0 || a.2 != b.2) {
        return Err("the standalone ledger changed; its originals are retained for another pass".into());
    }
    let directory = node.world.town(town);
    let mut originals = Vec::new();
    for name in ["signed", "ledger", "project.json", "projection-through.json", "refused.json", "index.json"] {
        archive::collect(node.world.path(), "@node", &directory.path().join(name), &mut originals)
            .map_err(|error| error.to_string())?;
    }
    let omitted: std::collections::BTreeSet<_> = entries
        .iter()
        .enumerate()
        .filter(|(index, _)| selected.binary_search(index).is_err())
        .map(|(_, (seq, _, _))| *seq)
        .collect();
    for original in &originals {
        if original.source().parent() == Some(directory.signed().as_path())
            && original
                .source()
                .file_stem()
                .and_then(|name| name.to_str())
                .and_then(|name| name.parse::<u64>().ok())
                .is_some_and(|seq| omitted.contains(&seq))
        {
            journal.retained.push(
                Retained::new(original.source(), &directory.path().join("not-imported"))
                    .map_err(|error| error.to_string())?,
            );
        }
    }
    originals.retain(|original| {
        !journal.retained.iter().any(|retained| retained.source() == original.source())
            && !journal
                .skipped
                .keys()
                .any(|name| original.source().starts_with(directory.ledger().join("history").join(name)))
    });
    journal.originals = Some(originals);
    fastctx_net::link::state::write(&path(&node.world, town), &journal).map_err(|error| error.to_string())?;
    finish(&node.world, town, &mut journal).map_err(|error| error.to_string())?;
    crate::context::invalidations::project(node, town);
    crate::replicas::updates::schedule(node, &journal.target.hub, crate::replicas::updates::Change::All);
    node.memory_changed();
    node.log
        .line(format_args!("carried {town} into {home}: {} entries; originals archived for 30 days", entries.len()));
    Ok(())
}

fn translated(node: &Node, link: &Link, entry: &Entry, own: &fastctx_net::sign::PublicKey) -> Result<Entry, String> {
    let here = node.machine();
    let old = &entry.author;
    let ours = entry.key == *own && *old != here;
    let renamed = |name: &MachineName| if ours && name == old { here.clone() } else { name.clone() };
    let mut files = Vec::new();
    for file in &entry.files {
        let bytes = if ours {
            Bytes(crate::town::rename::file(&file.path, &file.bytes.0, old, &here)?)
        } else {
            file.bytes.clone()
        };
        files.push(File { path: file.path.clone(), bytes });
    }
    if ours {
        crate::town::rename::memory(&mut files, old, &here)?;
    }
    history::stamp(entry, &mut files)?;
    let mut carried =
        link.entry(&entry.town, entry.machines.as_ref().map(|machines| machines.iter().map(renamed).collect()), files);
    carried.at = entry.at;
    Ok(carried)
}

async fn prefix(
    node: &Node,
    link: &Link,
    town: &TownId,
    source: &[(u64, Entry, Signed)],
    selected: &[usize],
    own: &fastctx_net::sign::PublicKey,
) -> Result<usize, String> {
    let mut through = 0usize;
    while through < selected.len() {
        let (page, more) = link.ledger_read(town, through as u64).await?;
        if page.is_empty() {
            if more {
                return Err("the hub returned an empty unfinished ledger page".into());
            }
            break;
        }
        for item in page {
            if through == selected.len() {
                break;
            }
            let entry = item.entry.ok_or_else(|| item.refused.unwrap_or_else(|| "unreadable carried entry".into()))?;
            let original = &source[selected[through]].1;
            let expected = translated(node, link, original, own)?;
            if item.seq != through as u64 + 1
                || !history::matches(original, &expected.files, &entry.files)
                || entry.at != expected.at
                || entry.machines != expected.machines
            {
                return Err(format!("the hub's ledger differs from the standalone source at entry {}", through + 1));
            }
            through += 1;
        }
        if !more {
            break;
        }
    }
    Ok(through)
}

fn report(node: &Node, town: &TownName) -> Result<(), String> {
    let Some(mut journal) = fastctx_net::link::state::read_or::<Option<Journal>>(&path(&node.world, town), None)
        .map_err(|error| error.to_string())?
    else {
        return Ok(());
    };
    if !journal.finished || journal.reported {
        return Ok(());
    }
    let directory = node.world.town(town);
    for (name, reason) in &journal.skipped {
        node.log.line(format_args!("project {town} history {name} was not imported: {reason}; signatures remain at {}, original files remain at {}",
            directory.path().join("not-imported").display(), directory.ledger().join("history").join(name).display()));
    }
    journal.reported = true;
    fastctx_net::link::state::write(&path(&node.world, town), &journal).map_err(|error| error.to_string())
}
