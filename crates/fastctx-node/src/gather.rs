//! Moves a World kept in the earlier layout into one directory per
//! conversation (`fastctx_types::layout`), when a node of this version first
//! starts on it, before anything else reads the records.
//!
//! The earlier layout kept a resident's records under the project it lived
//! in, one directory in each project it had lived in, the directory of each
//! project it left marked with where it went; a removed resident's records
//! under `archive/` in each; what a resident of another machine did here
//! beside them, named with its machine; and a few of its records in the
//! World's own directories. Each conversation is gathered on its own:
//!
//! 1. A plan names its earlier directories, most authoritative first, and
//!    the projects it was in; it is written into a hidden directory being
//!    filled, `conversations/.<name>.gathering`, before anything moves.
//! 2. Its files move in, one rename each. What a move between projects
//!    copied (what was on its way, receipts, turn records) is taken from the
//!    directory it lived in last; what stays where it happened (what it did,
//!    what it saw, what the user attached) from all of them. Files only
//!    copied, the lines of `actions`, `messages` and `seen`, are merged from
//!    sources that stay until the end.
//! 3. The hidden directory becomes the conversation's directory, its plan
//!    with it, and the earlier directories are emptied of what moved.
//!
//! A node stopped at any point finishes the same plan when it next starts.
//! Old paths handed to hosts or models remain as directory aliases into the
//! conversation. Running legacy command writers and Claude exchange writers
//! finish at their original paths before a later gather moves those directories
//! and installs their aliases. Their data stays counted and deleted with the
//! conversation throughout that transition.

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Path, PathBuf};

use fastctx_platform::{Permissions, ensure_private_dir, publish};
use fastctx_types::layout::{ConversationDir, Kind, UNKNOWN_MACHINE, WorldDir};
use fastctx_types::residents::{About, Moved, Places};
use fastctx_types::{Address, ResidentId, TownName};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

/// What gathering did, for the node's log.
#[derive(Default)]
pub struct Gathered {
    pub moved: Vec<String>,
    pub problems: Vec<(PathBuf, io::Error)>,
}

/// One earlier directory of a conversation.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Source {
    /// The earlier directory, including jobs and their output.
    directory: PathBuf,
    /// Where its records are: the directory, or its `archive` once removed.
    records: PathBuf,
}

#[derive(Debug, Serialize, Deserialize)]
struct Plan {
    /// Most authoritative first: the first is the one it lived in last.
    sources: Vec<Source>,
    /// The projects a resident was in; none for a guest room or a draft.
    places: Option<Places>,
    removed: Option<Timestamp>,
    /// Whether the settings a Claude Code session was started with stay
    /// where that session reads them.
    keeps_launch: bool,
}

/// Lines merged from every source rather than moved.
const LINES: &[&str] = &["actions", "messages", "seen"];

/// Files whose earlier paths need aliases after moving.
const JOB_FILES: &[&str] = &["jobs", "out", "artifacts-delivered.json"];
const LAUNCH_FILES: &[&str] = &["claude-settings.json", "claude-instructions.md"];

const PLAN: &str = ".plan.json";

/// Gathers every conversation of the earlier layout on this machine. A
/// conversation that cannot be gathered is reported and left as it was; the
/// next start tries it again.
pub fn gather(world: &WorldDir) -> io::Result<Gathered> {
    let mut gathered = Gathered::default();
    let earlier = world.earlier();
    let conversations = world.conversations();
    let mut homes: BTreeMap<ResidentId, Vec<(Option<TownName>, PathBuf)>> = BTreeMap::new();
    let mut guests: BTreeMap<Address, Vec<PathBuf>> = BTreeMap::new();
    for parent in earlier.parents()? {
        let town = earlier.town_of(&parent);
        for path in entries(&parent)? {
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else { continue };
            if !path.is_dir() || fastctx_platform::owned_directory::is_link(&std::fs::symlink_metadata(&path)?) {
                continue;
            }
            match Kind::of(name) {
                Some(Kind::Home(id)) => homes.entry(id).or_default().push((town.clone(), path)),
                Some(Kind::Guest(owner)) => guests.entry(owner).or_default().push(path),
                _ => {
                    gathered.problems.push((path, io::Error::other("not a conversation's directory; left where it is")))
                }
            }
        }
    }
    // What was left half done by a node that stopped while gathering.
    let mut pending = BTreeSet::new();
    for path in entries(&conversations)? {
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else { continue };
        if let Some(name) = name.strip_prefix('.').and_then(|name| name.strip_suffix(".gathering")) {
            // A node that stopped before its plan was written moved nothing
            // yet: it is planned again below.
            if path.join(PLAN).exists() {
                pending.insert(name.to_string());
            } else if let Err(error) = fastctx_platform::owned_directory::delete(&path) {
                gathered.problems.push((path.clone(), error));
            }
        } else if path.join(PLAN).exists() {
            pending.insert(name.to_string());
        }
    }
    let mut failed = BTreeSet::new();
    let mut placed: BTreeMap<ResidentId, Vec<PathBuf>> = BTreeMap::new();
    for (id, directories) in homes {
        if !pending.contains(id.as_str()) && conversations.join(id.as_str()).exists() {
            let directories: Vec<PathBuf> = directories.into_iter().map(|(_, path)| path).collect();
            left_over(&directories, &mut gathered)?;
            continue;
        }
        // A plan a node that stopped left is finished as it was made: what
        // moved already says nothing of where it came from.
        let result = if pending.remove(id.as_str()) {
            resume(world, id.as_str())
        } else {
            let removed = read_optional::<Timestamp>(&earlier.removed().join(format!("{id}.json")))?;
            match home_plan(&directories, removed) {
                Ok(Some(plan)) => gather_one(world, id.as_str(), plan),
                // No record says who it is: copies of files a conversation of
                // another machine placed here.
                Ok(None) => {
                    placed.entry(id).or_default().extend(directories.into_iter().map(|(_, path)| path));
                    continue;
                }
                Err(error) => Err(error),
            }
        };
        match result {
            Ok(()) => {
                gathered.moved.push(id.to_string());
                let removed = earlier.removed().join(format!("{id}.json"));
                if let Err(error) = remove_file(&removed) {
                    gathered.problems.push((removed, error));
                }
            }
            Err(error) => {
                gathered.problems.push((conversations.join(id.as_str()), error));
                failed.insert(id.to_string());
            }
        }
    }
    for (owner, directories) in guests {
        let name = owner.key();
        if !pending.contains(&name) && conversations.join(&name).exists() {
            left_over(&directories, &mut gathered)?;
            continue;
        }
        let result = if pending.remove(&name) {
            resume(world, &name)
        } else {
            let sources =
                directories.into_iter().map(|directory| Source { records: directory.clone(), directory }).collect();
            gather_one(world, &name, Plan { sources, places: None, removed: None, keeps_launch: false })
        };
        match result {
            Ok(()) => gathered.moved.push(name),
            Err(error) => {
                gathered.problems.push((conversations.join(&name), error));
                failed.insert(name);
            }
        }
    }
    // A plan whose earlier directories are gone already: what is left is to
    // finish it.
    for name in pending {
        match resume(world, &name) {
            Ok(()) => gathered.moved.push(name),
            Err(error) => {
                gathered.problems.push((conversations.join(&name), error));
                failed.insert(name);
            }
        }
    }
    for (id, directories) in placed {
        match place_copies(world, &id, &directories) {
            Ok(Some(room)) => gathered.moved.push(room),
            Ok(None) => {}
            Err(error) => gathered.problems.push((directories[0].clone(), error)),
        }
    }
    // What cannot be moved or removed now, a directory another program
    // holds on Windows among it, is told and tried again at the next start:
    // it never keeps the node from starting.
    if let Err(error) = gather_books(world, &failed, &mut gathered) {
        gathered.problems.push((world.path().to_path_buf(), error));
    }
    if let Err(error) = gather_queues(world, &failed, &mut gathered) {
        gathered.problems.push((world.path().to_path_buf(), error));
    }
    for kept in world.kept()? {
        let aliases = read_optional::<BTreeMap<PathBuf, PathBuf>>(&kept.directory.aliases())?.unwrap_or_default();
        for (alias, destination) in aliases {
            if destination.is_dir()
                && !alias.exists()
                && let Err(error) = fastctx_platform::directory_alias::create(&destination, &alias)
            {
                gathered.problems.push((alias, error));
            }
        }
    }
    for session in entries(&fastctx_hosts::claude::exchange::sessions(world.path()))? {
        if let Err(error) = gather_session(world, &session) {
            gathered.problems.push((session, error));
        }
    }
    if let Err(error) = sweep(world) {
        gathered.problems.push((world.path().to_path_buf(), error));
    }
    Ok(gathered)
}

/// A Claude session's directory left outside its conversation, as a session
/// that was live at the last start left it: moved in now.
fn gather_session(world: &WorldDir, session: &Path) -> io::Result<()> {
    if fastctx_platform::owned_directory::is_link(&std::fs::symlink_metadata(session)?) {
        return Ok(());
    }
    if let Some(id) = read_optional::<ResidentId>(&session.join("resident.json"))?
        && world.conversation(&id).about().exists()
        && let Some(name) = session.file_name().and_then(|name| name.to_str())
    {
        claude_room(world, &id, name)?;
    }
    Ok(())
}

/// The plan of a resident of this machine, from its earlier directories;
/// `None` when none of them says who it is and none holds a draft.
pub(crate) fn claude_room(world: &WorldDir, id: &ResidentId, session: &str) -> io::Result<()> {
    crate::claude::pairing::validate_session(session)?;
    let home = world.conversation(id);
    let source = fastctx_hosts::claude::exchange::sessions(world.path()).join(session);
    if source.exists() && fastctx_platform::owned_directory::is_link(&std::fs::symlink_metadata(&source)?) {
        return fastctx_platform::directory_alias::create(&home.claude_exchange(session), &source);
    }
    if let Some(registration) = fastctx_hosts::claude::exchange::registration(&source)?
        && let Some(process) = registration.process
        && fastctx_platform::is_alive(&process)?
    {
        // The mod publishes through this path without the node's lock.
        // Its directory moves only after that writer has exited.
        return Ok(());
    }
    let control = source.join("control");
    if let Some(host) = fastctx_hosts::claude::control::host(&control)?
        && fastctx_platform::is_alive(&host.process)?
    {
        return Ok(());
    }
    #[derive(Deserialize)]
    struct Helper {
        process: fastctx_types::ProcessIdentity,
    }
    for request in fastctx_hosts::claude::control::requests(&control)? {
        if let Some(helper) = read_optional::<Helper>(&request.join("request.json"))?
            && fastctx_platform::is_alive(&helper.process)?
        {
            return Ok(());
        }
    }
    let target = home.claude_exchange(session);
    ensure_private_dir(target.parent().expect("Claude exchange parent"))?;
    let mut aliases = read_optional::<BTreeMap<PathBuf, PathBuf>>(&home.aliases())?.unwrap_or_default();
    aliases.insert(source.clone(), target.clone());
    publish(&home.aliases(), &json(&aliases)?, Permissions::Private)?;
    if !target.exists() {
        fastctx_platform::owned_directory::move_to(&source, &target)?;
    } else if source.exists() {
        move_record(&source, &target, &home.path().join("recovered").join("claude").join(session))?;
        fastctx_platform::owned_directory::delete(&source)?;
    }
    if !source.exists() {
        fastctx_platform::directory_alias::create(&target, &source)?;
    }
    Ok(())
}

fn home_plan(directories: &[(Option<TownName>, PathBuf)], removed: Option<Timestamp>) -> io::Result<Option<Plan>> {
    struct Found {
        town: Option<TownName>,
        source: Source,
        about: Option<About>,
        moved: Option<Moved>,
        draft: bool,
    }
    let mut found = Vec::new();
    for (town, directory) in directories {
        let archived = directory.join("archive");
        let records = if archived.join("about").exists() { archived } else { directory.clone() };
        let about = read_optional::<About>(&records.join("about"))?;
        let moved = read_optional::<Moved>(&records.join("moved"))?;
        let draft = records.join("attachment-draft.json").exists();
        found.push(Found {
            town: town.clone(),
            source: Source { directory: directory.clone(), records },
            about,
            moved,
            draft,
        });
    }
    let Some(about) = found.iter().find_map(|found| found.about.clone()) else {
        return Ok(found.iter().find(|found| found.draft).map(|draft| Plan {
            sources: std::iter::once(draft.source.clone())
                .chain(
                    found
                        .iter()
                        .filter(|found| found.source.directory != draft.source.directory)
                        .map(|found| found.source.clone()),
                )
                .collect(),
            places: None,
            removed,
            keeps_launch: false,
        }));
    };
    // Where it lives now: the directory with a record that does not say it
    // moved; a move cut short left none such, and it lives where the latest
    // move went.
    let unmarked: Vec<usize> =
        (0..found.len()).filter(|&index| found[index].about.is_some() && found[index].moved.is_none()).collect();
    let (live, town) = match unmarked.as_slice() {
        [index] => (*index, found[*index].town.clone()),
        [] => {
            let latest = (0..found.len())
                .filter(|&index| found[index].about.is_some())
                .max_by_key(|&index| found[index].moved.as_ref().map(|moved| moved.at))
                .expect("one record says who it is");
            let to = found[latest].moved.as_ref().and_then(|moved| moved.to.clone());
            (latest, to)
        }
        several => {
            // A failed old move may leave both copies unmarked. The latest
            // state wins; differing older records are retained in recovered/.
            let index = *several
                .iter()
                .max_by_key(|&&index| {
                    std::fs::metadata(found[index].source.records.join("now"))
                        .and_then(|metadata| metadata.modified())
                        .ok()
                })
                .expect("nonempty candidates");
            (index, found[index].town.clone())
        }
    };
    // Records first, the latest move first; directories a move left without
    // a record last.
    let mut order: Vec<usize> = (0..found.len()).filter(|&index| index != live).collect();
    order.sort_by_key(|&index| {
        (found[index].about.is_none(), std::cmp::Reverse(found[index].moved.as_ref().map(|moved| moved.at)))
    });
    let mut sources = Vec::new();
    for index in std::iter::once(live).chain(order) {
        let source = found[index].source.clone();
        sources.push(source.clone());
        if source.records != source.directory {
            // An interrupted removal can leave half its records unarchived.
            sources.push(Source { records: source.directory.clone(), directory: source.directory });
        } else if source.directory.join("archive").is_dir() {
            sources.push(Source { records: source.directory.join("archive"), directory: source.directory });
        }
    }
    // The projects it was in, from the moves its directories recorded. A
    // return to a project it had left erased that departure; such a stay is
    // given to the project it was in next.
    let mut departures: Vec<(Option<TownName>, Moved)> = found
        .iter()
        .filter(|found| found.about.is_some())
        .filter_map(|found| Some((found.town.clone(), found.moved.clone()?)))
        .collect();
    departures.sort_by_key(|(_, moved)| moved.at);
    let first = departures.first().map_or_else(|| town.clone(), |(from, _)| from.clone());
    let mut places = Places::born(first, about.born_at);
    for (_, moved) in &departures {
        places.enter(moved.to.clone(), moved.at);
    }
    if places.now() != town.as_ref() {
        let at = departures.last().map_or(about.born_at, |(_, moved)| moved.at);
        places.enter(town, at);
    }
    let keeps_launch = removed.is_none() && about.world_started && about.host.agent.as_str() == "claude";
    Ok(Some(Plan { sources, places: Some(places), removed, keeps_launch }))
}

/// Shared legacy queues contain an owner in each pending JSON record. Opaque
/// receipt names alone cannot recover that owner, so those remain readable by
/// the compatibility lookup rather than being guessed or discarded.
fn gather_queues(world: &WorldDir, failed: &BTreeSet<String>, gathered: &mut Gathered) -> io::Result<()> {
    let own = crate::machine::names(world)?;
    let here = fastctx_types::layout::Names { me: own.name.clone(), was: own.was.clone(), renamed: Vec::new() };
    for name in ["job-deliveries", "on-deliveries", "on-stops"] {
        let shared = world.path().join(name);
        for path in entries(&shared)? {
            if path.extension().is_none_or(|extension| extension != "json") {
                continue;
            }
            let value: serde_json::Value = read_json(&path)?;
            let owner: Address = match name {
                "on-deliveries" => Address {
                    machine: own.name.clone(),
                    resident: serde_json::from_value(value["to"].clone()).map_err(io::Error::other)?,
                },
                "on-stops" => serde_json::from_value(value["origin"]["resident"].clone()).map_err(io::Error::other)?,
                _ => serde_json::from_value(value["owner"].clone()).map_err(io::Error::other)?,
            };
            let name_of_owner = if own.is(&owner.machine) { owner.resident.to_string() } else { owner.key() };
            if failed.contains(&name_of_owner) {
                continue;
            }
            let directory = world.owner(&here, &owner).book(name);
            for extension in ["taken", "stopped"] {
                let receipt = path.with_extension(extension);
                if receipt.exists() {
                    move_one(&receipt, &directory.join(receipt.file_name().expect("queue record")))?;
                    remove_file(&receipt)?;
                }
            }
            move_one(&path, &directory.join(path.file_name().expect("queue record")))?;
            remove_file(&path)?;
        }
        let remaining = entries(&shared)?;
        if remaining.is_empty() {
            remove_dir(&shared)?;
        } else if name == "on-deliveries" {
            gathered.problems.push((shared, io::Error::other("legacy receipts have no recoverable owner; retained")));
        }
    }
    Ok(())
}

/// The earlier directories of a conversation gathered before hold only what
/// stays there; anything else in them is reported and left alone.
fn left_over(directories: &[PathBuf], gathered: &mut Gathered) -> io::Result<()> {
    for directory in directories {
        let unexpected: Vec<String> = entries(directory)?
            .iter()
            .filter_map(|entry| entry.file_name()?.to_str().map(str::to_string))
            .filter(|name| !JOB_FILES.contains(&name.as_str()) && !LAUNCH_FILES.contains(&name.as_str()))
            .collect();
        if !unexpected.is_empty() {
            gathered.problems.push((
                directory.clone(),
                io::Error::other(format!(
                    "its conversation was gathered before, and {} appeared here since; left where it is",
                    unexpected.join(", ")
                )),
            ));
        }
    }
    Ok(())
}

/// Gathers one conversation by a new plan.
fn gather_one(world: &WorldDir, name: &str, plan: Plan) -> io::Result<()> {
    let conversations = world.conversations();
    let target = conversations.join(name);
    let staging = conversations.join(format!(".{name}.gathering"));
    if target.exists() {
        return Err(io::Error::other(format!(
            "{} exists already; the earlier records of {name} were left where they are",
            target.display()
        )));
    }
    ensure_private_dir(&staging)?;
    publish(&staging.join(PLAN), &serde_json::to_vec_pretty(&plan).map_err(io::Error::other)?, Permissions::Private)?;
    fill(world, name)
}

/// Finishes the plan a node that stopped left for `name`.
fn resume(world: &WorldDir, name: &str) -> io::Result<()> {
    let staging = world.conversations().join(format!(".{name}.gathering"));
    if staging.join(PLAN).exists() {
        return fill(world, name);
    }
    clean(world, name)
}

/// Moves a conversation's records into the hidden directory its plan is in,
/// makes that its directory and empties the earlier ones.
fn fill(world: &WorldDir, name: &str) -> io::Result<()> {
    let conversations = world.conversations();
    let staging = conversations.join(format!(".{name}.gathering"));
    let plan: Plan = read_json(&staging.join(PLAN))?;
    let into = ConversationDir::at(&staging);
    for (index, source) in plan.sources.iter().enumerate() {
        let relative = if source.records == source.directory {
            PathBuf::from("attachments")
        } else {
            PathBuf::from("archive/attachments")
        };
        let index = plan.sources.iter().position(|first| first.directory == source.directory).unwrap_or(index);
        preserve_paths(
            &source.records.join("attachments"),
            &staging.join("legacy").join(index.to_string()).join(relative),
        )?;
    }
    for (index, source) in plan.sources.iter().enumerate() {
        move_records(source, &into, index, plan.keeps_launch)?;
    }
    merge_lines(&plan.sources, &into)?;
    if let Some(places) = &plan.places {
        publish(&into.town(), &json(places)?, Permissions::Private)?;
    }
    if let Some(removed) = &plan.removed {
        publish(&into.removed(), &serde_json::to_vec(removed).map_err(io::Error::other)?, Permissions::Private)?;
    }
    let mut aliases = BTreeMap::new();
    for (index, source) in plan.sources.iter().enumerate() {
        if aliases.contains_key(&source.directory) {
            continue;
        }
        let legacy = staging.join("legacy").join(index.to_string());
        for file in JOB_FILES.iter().chain(LAUNCH_FILES) {
            let path = source.directory.join(file);
            if path.exists() && !live_jobs(&source.directory)? {
                move_one(&path, &legacy.join(file))?;
            }
        }
        // A restart after a rename still knows which old paths need aliases.
        if legacy.exists() {
            aliases.insert(source.directory.clone(), conversations.join(name).join("legacy").join(index.to_string()));
        }
    }
    publish(&staging.join("aliases.json"), &json(&aliases)?, Permissions::Private)?;
    fastctx_platform::owned_directory::move_to(&staging, &conversations.join(name))?;
    clean(world, name)
}

/// Empties the earlier directories of a gathered conversation of what moved
/// and forgets its plan.
fn clean(world: &WorldDir, name: &str) -> io::Result<()> {
    let target = world.conversations().join(name);
    let plan: Plan = read_json(&target.join(PLAN))?;
    for source in &plan.sources {
        if source.directory.exists()
            && !fastctx_platform::owned_directory::is_link(&std::fs::symlink_metadata(&source.directory)?)
            && live_jobs(&source.directory)?
        {
            return Ok(());
        }
    }
    // Jobs left running during the first pass move only after their writer
    // released the record lock; the next node start finishes this plan.
    let mut aliases: BTreeMap<PathBuf, PathBuf> = read_json(&target.join("aliases.json"))?;
    for (index, source) in plan.sources.iter().enumerate() {
        if source.directory.exists()
            && !fastctx_platform::owned_directory::is_link(&std::fs::symlink_metadata(&source.directory)?)
        {
            let legacy = target.join("legacy").join(index.to_string());
            for file in JOB_FILES.iter().chain(LAUNCH_FILES) {
                let path = source.directory.join(file);
                if path.exists() {
                    move_one(&path, &legacy.join(file))?;
                }
            }
            if legacy.exists() {
                aliases.entry(source.directory.clone()).or_insert(legacy);
            }
        }
    }
    publish(&target.join("aliases.json"), &json(&aliases)?, Permissions::Private)?;
    for source in &plan.sources {
        if source.directory.exists()
            && fastctx_platform::owned_directory::is_link(&std::fs::symlink_metadata(&source.directory)?)
        {
            continue;
        }
        for entry in entries(&source.directory)? {
            fastctx_platform::owned_directory::delete(&entry)?;
        }
        if entries(&source.directory)?.is_empty() {
            remove_dir(&source.directory)?;
        }
    }
    for (alias, destination) in aliases {
        fastctx_platform::directory_alias::create(&destination, &alias)?;
    }
    remove_file(&target.join(PLAN))
}

fn live_jobs(directory: &Path) -> io::Result<bool> {
    for path in entries(&directory.join("jobs"))? {
        if path.extension().is_some_and(|extension| extension == "json") {
            let record = fastctx_hands::run::record::read(&path)?;
            if let Some(supervisor) = &record.supervisor
                && fastctx_platform::is_alive(supervisor)?
            {
                return Ok(true);
            }
            if fastctx_hands::run::program::in_use(&record)? {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// Keeps handed-out attachment paths without copying bytes. Each source's
/// independent file identity survives even when matching records are merged.
fn preserve_paths(from: &Path, to: &Path) -> io::Result<()> {
    let metadata = match std::fs::symlink_metadata(from) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if fastctx_platform::owned_directory::is_link(&metadata) {
        return Err(io::Error::other("an attachment migration source is a filesystem link"));
    }
    if metadata.is_dir() {
        ensure_private_dir(to)?;
        for child in entries(from)? {
            preserve_paths(&child, &to.join(child.file_name().expect("named")))?;
        }
    } else if !to.exists() {
        ensure_private_dir(to.parent().expect("attachment parent"))?;
        std::fs::hard_link(from, to)?;
    }
    Ok(())
}

/// Moves the records of one earlier directory into `into`. The first source
/// gives what a move between projects carried; every source gives the rest,
/// each name taken from the first source that has it.
fn move_records(source: &Source, into: &ConversationDir, index: usize, keeps_launch: bool) -> io::Result<()> {
    for entry in entries(&source.records)? {
        let Some(name) = entry.file_name().and_then(|name| name.to_str()).map(str::to_string) else { continue };
        if matches!(name.as_str(), "moved" | "archive" | "delivered" | "session-events-sequence.json")
            || LINES.contains(&name.as_str())
            || JOB_FILES.contains(&name.as_str())
            || (keeps_launch && LAUNCH_FILES.contains(&name.as_str()))
        {
            continue;
        }
        let destination = destination(into, &name);
        if destination.exists() && matches!(name.as_str(), "about" | "now" | "composer.json") {
            if std::fs::read(&entry)? != std::fs::read(&destination)? {
                move_one(&entry, &into.path().join("recovered").join(index.to_string()).join(&name))?;
            }
            continue;
        }
        if name == "outbox" {
            for answer in entries(&entry)? {
                let Some(file) = answer.file_name().and_then(|name| name.to_str()).map(str::to_string) else {
                    continue;
                };
                match file.strip_suffix(".paid") {
                    Some(id) => move_record(
                        &answer,
                        &into.paid().join(id),
                        &into.path().join("recovered").join(index.to_string()).join("paid").join(id),
                    )?,
                    None => move_record(
                        &answer,
                        &destination.join(&file),
                        &into.path().join("recovered").join(index.to_string()).join(&name).join(&file),
                    )?,
                }
            }
        } else if entry.is_dir() && !fastctx_platform::owned_directory::is_link(&std::fs::symlink_metadata(&entry)?) {
            for child in entries(&entry)? {
                let file = child.file_name().expect("a directory entry has a name").to_owned();
                move_record(
                    &child,
                    &destination.join(&file),
                    &into.path().join("recovered").join(index.to_string()).join(&name).join(file),
                )?;
            }
        } else {
            move_record(&entry, &destination, &into.path().join("recovered").join(index.to_string()).join(&name))?;
        }
    }
    Ok(())
}

/// Merges directories and keeps a differing older file at its source-relative
/// recovery path. An existing recovery record must match before cleanup.
fn move_record(from: &Path, to: &Path, recovered: &Path) -> io::Result<()> {
    if !to.exists() {
        return move_one(from, to);
    }
    let source = std::fs::symlink_metadata(from)?;
    let target = std::fs::symlink_metadata(to)?;
    if source.is_dir()
        && target.is_dir()
        && !fastctx_platform::owned_directory::is_link(&source)
        && !fastctx_platform::owned_directory::is_link(&target)
    {
        for child in entries(from)? {
            let name = child.file_name().expect("named entry");
            move_record(&child, &to.join(name), &recovered.join(name))?;
        }
        return Ok(());
    }
    if !source.is_dir() && !target.is_dir() && std::fs::read(from)? == std::fs::read(to)? {
        return Ok(());
    }
    move_one(from, recovered)
}

/// Where a name of the earlier layout goes in a conversation's directory.
fn destination(into: &ConversationDir, name: &str) -> PathBuf {
    match name {
        "delivery-receipts" => into.delivery_receipts(),
        "turn-sessions" => into.turn_sessions(),
        "composer-receipts" => into.composer_books("receipts"),
        "composer-uploads" => into.composer_books("uploads"),
        "composer-ended" => into.composer_books("ended"),
        "turn-facts" => into.turn_facts(),
        _ => into.path().join(name),
    }
}

/// Moves `from` to `to` unless `to` is there already, from a source given
/// first: then `from` is a copy a move between projects made earlier, and
/// it stays to be removed with the rest of its directory.
fn move_one(from: &Path, to: &Path) -> io::Result<()> {
    if to.exists() {
        let source = std::fs::symlink_metadata(from)?;
        let target = std::fs::symlink_metadata(to)?;
        if source.is_dir()
            && target.is_dir()
            && !fastctx_platform::owned_directory::is_link(&source)
            && !fastctx_platform::owned_directory::is_link(&target)
        {
            for child in entries(from)? {
                move_one(&child, &to.join(child.file_name().expect("named entry")))?;
            }
            return Ok(());
        }
        if !source.is_dir() && !target.is_dir() && std::fs::read(from)? == std::fs::read(to)? {
            return Ok(());
        }
        return Err(io::Error::other(format!(
            "conflicting migration records: {} and {}",
            from.display(),
            to.display()
        )));
    }
    ensure_private_dir(to.parent().expect("a destination has a directory"))?;
    fastctx_platform::owned_directory::move_to(from, to)
}

/// Writes the lines of `actions`, `messages` and `seen` of every source in
/// the order they happened, the list of what went into its session, and the
/// next number of its session events.
fn merge_lines(sources: &[Source], into: &ConversationDir) -> io::Result<()> {
    for name in LINES {
        let mut lines: Vec<(Timestamp, String)> = Vec::new();
        let mut seen = BTreeSet::new();
        for source in sources {
            let text = match std::fs::read_to_string(source.records.join(name)) {
                Ok(text) => text,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            };
            let mut last = Timestamp::UNIX_EPOCH;
            for line in text.lines().filter(|line| !line.trim().is_empty()) {
                // A move appended what it had seen to the new directory: the
                // same line twice is one.
                if !seen.insert(line.to_string()) {
                    continue;
                }
                let at = serde_json::from_str::<serde_json::Value>(line)
                    .ok()
                    .and_then(|value| value.get("at")?.as_str()?.parse::<Timestamp>().ok())
                    .unwrap_or(last);
                last = at;
                lines.push((at, line.to_string()));
            }
        }
        if lines.is_empty() {
            continue;
        }
        lines.sort_by_key(|(at, _)| *at);
        let mut bytes = Vec::new();
        for (_, line) in lines {
            bytes.extend_from_slice(line.as_bytes());
            bytes.push(b'\n');
        }
        let path = if *name == "seen" { into.seen() } else { into.path().join(name) };
        ensure_private_dir(path.parent().expect("a record has a directory"))?;
        publish(&path, &bytes, Permissions::Private)?;
    }
    let mut delivered = BTreeSet::new();
    let mut sequence: Option<u64> = None;
    for source in sources {
        match std::fs::read_to_string(source.records.join("delivered")) {
            Ok(text) => delivered.extend(text.lines().filter(|line| !line.is_empty()).map(str::to_string)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        if let Some(number) = read_optional::<u64>(&source.records.join("session-events-sequence.json"))? {
            sequence = Some(sequence.map_or(number, |prior| prior.max(number)));
        }
    }
    if !delivered.is_empty() {
        ensure_private_dir(&into.books())?;
        let text = delivered.into_iter().collect::<Vec<_>>().join("\n") + "\n";
        publish(&into.delivered(), text.as_bytes(), Permissions::Private)?;
    }
    if let Some(sequence) = sequence {
        publish(
            &into.session_sequence(),
            &serde_json::to_vec(&sequence).map_err(io::Error::other)?,
            Permissions::Private,
        )?;
        crate::session_record::reset(&into.session_sequence());
    }
    Ok(())
}

/// Moves copies of files a conversation of another machine placed here
/// before the machine they came from was recorded: into its guest room when
/// one conversation of that id worked here, else into one of unknown origin.
/// Returns the room's name, or `None` when there were none.
fn place_copies(world: &WorldDir, id: &ResidentId, directories: &[PathBuf]) -> io::Result<Option<String>> {
    let movable = |directory: &PathBuf| -> io::Result<bool> {
        Ok(entries(directory)?.iter().any(|entry| {
            entry.file_name().and_then(|name| name.to_str()).is_some_and(|name| !JOB_FILES.contains(&name))
        }))
    };
    let mut any = false;
    for directory in directories {
        any |= movable(directory)?;
    }
    if !any {
        return Ok(None);
    }
    let mut machines: Vec<String> = Vec::new();
    for kept in world.kept()? {
        if let Kind::Guest(owner) = &kept.kind
            && owner.resident == *id
        {
            machines.push(kept.name.clone());
        }
    }
    let name = match machines.as_slice() {
        [room] => room.clone(),
        _ => format!("{id}@{UNKNOWN_MACHINE}"),
    };
    let room = ConversationDir::at(&world.conversations().join(&name));
    ensure_private_dir(&room.attachments())?;
    for directory in directories {
        for slot in entries(&directory.join("attachments"))? {
            let Some(slot_name) = slot.file_name().and_then(|name| name.to_str()).map(str::to_string) else {
                continue;
            };
            // The same file placed under two projects keeps both copies.
            let mut destination = room.attachments().join(&slot_name);
            let mut n = 2;
            while destination.exists() {
                destination = room.attachments().join(format!("{slot_name}-{n}"));
                n += 1;
            }
            fastctx_platform::owned_directory::move_to(&slot, &destination)?;
        }
        remove_dir(&directory.join("attachments"))?;
        for entry in entries(directory)? {
            let name = entry.file_name().and_then(|name| name.to_str()).unwrap_or_default().to_string();
            if !JOB_FILES.contains(&name.as_str()) {
                move_one(&entry, &room.path().join(&name))?;
            }
        }
        if entries(directory)?.is_empty() {
            remove_dir(directory)?;
        }
    }
    Ok(Some(name))
}

/// Moves what the earlier layout kept of each conversation in the World's
/// own directories into its books. A conversation whose gathering failed
/// keeps them where they are, so nothing is made in its name before its
/// records are.
fn gather_books(world: &WorldDir, failed: &BTreeSet<String>, gathered: &mut Gathered) -> io::Result<()> {
    let earlier = world.earlier();
    let codex = earlier.codex();
    let mut sessions: BTreeMap<String, ResidentId> = BTreeMap::new();
    for kept in world.kept()? {
        if let Kind::Home(id) = &kept.kind
            && let Some(about) = read_optional::<About>(&kept.directory.about())?
            && let Some(session) = about.session
        {
            sessions.insert(session, id.clone());
        }
    }
    let ready = |id: &str| !failed.contains(id) && ResidentId::try_from(id).is_ok();
    for path in entries(&world.path().join("codex-reports"))? {
        if path.extension().is_some_and(|extension| extension == "json") {
            #[derive(Deserialize)]
            struct Report {
                thread: String,
            }
            let report: Report = read_json(&path)?;
            if let Some(id) = sessions.get(&report.thread) {
                if ready(id.as_str()) {
                    move_one(&path, &world.conversation(id).books().join("host-reports.json"))?;
                    remove_file(&path)?;
                }
            } else {
                remove_file(&path)?;
            }
        } else if path.extension().is_some_and(|extension| extension == "jsonl") {
            let claimed = sessions.iter().find(|(thread, _)| {
                path.file_stem().and_then(|name| name.to_str())
                    == Some(crate::speech::fixed_id("thread", &[thread]).as_str())
            });
            if let Some((_, id)) = claimed {
                if ready(id.as_str()) {
                    move_one(&path, &world.conversation(id).books().join("host-reports.jsonl"))?;
                    remove_file(&path)?;
                }
            } else {
                remove_file(&path)?;
            }
        }
    }
    remove_dir(&world.path().join("codex-reports"))?;
    for (from, file) in [("options", "options.json"), ("callees", "callee.json")] {
        for path in entries(&codex.join(from))? {
            let Some(id) = path.file_stem().and_then(|stem| stem.to_str()).map(str::to_string) else { continue };
            if ready(&id) {
                move_one(&path, &world.conversations().join(&id).join("books").join(file))?;
                remove_file(&path)?;
            }
        }
        remove_dir(&codex.join(from))?;
    }
    for asker in entries(&codex.join("asked"))? {
        let Some(id) = asker.file_name().and_then(|name| name.to_str()).map(str::to_string) else { continue };
        if !ready(&id) {
            continue;
        }
        let asked = world.conversation(&ResidentId::try_from(id.as_str()).expect("checked")).asked();
        for machine in entries(&asker)? {
            for file in entries(&machine)? {
                let to = asked.join(machine.file_name().expect("named")).join(file.file_name().expect("named"));
                move_one(&file, &to)?;
                remove_file(&file)?;
            }
            remove_dir(&machine)?;
        }
        remove_dir(&asker)?;
    }
    remove_dir(&codex.join("asked"))?;
    let mut choices = vec![codex.join("choices")];
    for home in entries(&codex.join("homes"))? {
        choices.push(home.join("choices"));
    }
    for directory in choices {
        for path in entries(&directory)? {
            let thread = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .and_then(|hex| {
                    (0..hex.len())
                        .step_by(2)
                        .map(|at| u8::from_str_radix(hex.get(at..at + 2)?, 16).ok())
                        .collect::<Option<Vec<u8>>>()
                })
                .and_then(|bytes| String::from_utf8(bytes).ok());
            match thread.and_then(|thread| sessions.get(&thread)) {
                Some(id) if ready(id.as_str()) => {
                    move_one(&path, &world.conversation(id).choices())?;
                    remove_file(&path)?;
                }
                Some(_) => {}
                None => gathered
                    .problems
                    .push((path, io::Error::other("the choices of a Codex thread no conversation here has; left"))),
            }
        }
        remove_dir(&directory)?;
    }
    for resident in entries(&earlier.elsewhere())? {
        let Some(id) = resident.file_name().and_then(|name| name.to_str()).map(str::to_string) else { continue };
        if !ready(&id) {
            continue;
        }
        let into = world.conversation(&ResidentId::try_from(id.as_str()).expect("checked")).elsewhere();
        for file in entries(&resident)? {
            move_one(&file, &into.join(file.file_name().expect("named")))?;
            remove_file(&file)?;
        }
        remove_dir(&resident)?;
    }
    remove_dir(&earlier.elsewhere())?;
    for tombstone in entries(&earlier.removed())? {
        let gathered_removal = tombstone
            .file_stem()
            .and_then(|stem| stem.to_str())
            .is_some_and(|id| world.conversations().join(id).join("removed").exists());
        if gathered_removal {
            remove_file(&tombstone)?;
        } else {
            gathered
                .problems
                .push((tombstone, io::Error::other("the removal of a resident with no records here; left")));
        }
    }
    remove_dir(&earlier.removed())?;
    Ok(())
}

/// Removes the directories of the earlier layout nothing is left in: those
/// whose jobs went as their output expired, and the projects' `residents`
/// once empty. `outside` itself stays: sessions outside every project stand
/// there.
pub fn sweep(world: &WorldDir) -> io::Result<()> {
    let earlier = world.earlier();
    for parent in earlier.parents()? {
        for directory in entries(&parent)? {
            if directory.is_dir()
                && !fastctx_platform::owned_directory::is_link(&std::fs::symlink_metadata(&directory)?)
                && holds_no_file(&directory)?
            {
                fastctx_platform::owned_directory::delete(&directory)?;
            }
        }
        remove_dir(&parent)?;
    }
    Ok(())
}

fn holds_no_file(directory: &Path) -> io::Result<bool> {
    for entry in entries(directory)? {
        let metadata = std::fs::symlink_metadata(&entry)?;
        if !metadata.is_dir() || fastctx_platform::owned_directory::is_link(&metadata) || !holds_no_file(&entry)? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Removes a directory if it is empty; one that is not, or is gone, stays as
/// it is.
fn remove_dir(path: &Path) -> io::Result<()> {
    match std::fs::remove_dir(path) {
        Err(error) if error.kind() != io::ErrorKind::NotFound && error.kind() != io::ErrorKind::DirectoryNotEmpty => {
            Err(error)
        }
        _ => Ok(()),
    }
}

fn remove_file(path: &Path) -> io::Result<()> {
    match std::fs::remove_file(path) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

fn entries(path: &Path) -> io::Result<Vec<PathBuf>> {
    match std::fs::read_dir(path) {
        Ok(entries) => {
            let mut paths = entries.map(|entry| entry.map(|entry| entry.path())).collect::<io::Result<Vec<_>>>()?;
            paths.sort();
            Ok(paths)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error),
    }
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> io::Result<T> {
    serde_json::from_slice(&std::fs::read(path)?)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, format!("{}: {error}", path.display())))
}

fn read_optional<T: serde::de::DeserializeOwned>(path: &Path) -> io::Result<Option<T>> {
    match read_json(path) {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn json(value: &impl Serialize) -> io::Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(io::Error::other)?;
    bytes.push(b'\n');
    Ok(bytes)
}
