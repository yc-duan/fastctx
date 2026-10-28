//! Measures what the World keeps on this machine: by kind, and by
//! conversation with its parts. Sizes are the bytes of the files and of the
//! directory entries themselves, links not followed: what `du -sb` counts.

use std::collections::{BTreeMap, HashMap};
use std::io;
use std::path::{Path, PathBuf};

use fastctx_types::data::{ClaudeKeeping, ConversationSpace, Group, HostRecord, Kinds, Parts, Space};
use fastctx_types::layout::{ConversationDir, Kind};
use fastctx_types::residents::Activity;
use fastctx_types::{AgentName, MachineName, ResidentId, TownName};
use jiff::Timestamp;

use super::host;
use crate::Node;

/// Bytes, and when the latest of the files counted changed.
#[derive(Clone, Copy, Default)]
pub(super) struct Size {
    pub bytes: u64,
    pub latest: Option<Timestamp>,
}

impl Size {
    fn add(&mut self, other: Size) {
        self.bytes += other.bytes;
        self.latest = self.latest.max(other.latest);
    }
}

/// What one conversation keeps here, before it is reported.
#[derive(Default)]
pub(super) struct Measured {
    pub records: Size,
    pub output: Size,
    pub host: Size,
    /// Every place that holds something of it here, its directory first.
    pub places: Vec<PathBuf>,
    /// What its agent's program keeps of it.
    pub host_files: Vec<PathBuf>,
    /// The sessions its agent's program knew it by: Claude Code's sessions,
    /// or its Codex thread.
    pub sessions: Vec<String>,
    pub host_record: HostRecord,
    pub claude_home: Option<PathBuf>,
    /// The jobs of it that still run here.
    pub running: Vec<String>,
    /// Records whose running state could not be established; never proof of idleness.
    pub unreadable_jobs: Vec<String>,
    /// The project of the latest job it ran here.
    pub job_town: Option<(Timestamp, Option<TownName>)>,
}

pub(super) fn measure(node: &Node) -> io::Result<Space> {
    let world = &node.world;
    let mut measured = conversations(node)?;
    let mut kinds = Kinds::default();
    let root = &node.root;
    let canonical_root = std::fs::canonicalize(root)?;
    let mut owned = Vec::new();
    for one in measured.values() {
        for path in &one.places {
            owned.push(path.clone());
        }
        for path in &one.host_files {
            if std::fs::canonicalize(path)?.starts_with(&canonical_root) {
                owned.push(path.clone());
            } else {
                kinds.host_records += tally(path)?.bytes;
            }
        }
    }
    let programs =
        [root.join("bin"), root.join("renewal"), world.path().join("bin"), world.path().join("claude-plugins")];
    let cache = root.join("cache");
    let viewed = cache.join("viewed");
    let mut root_bytes = 0;
    fastctx_platform::disk_usage::visit(root, |path, _, bytes| {
        root_bytes += bytes;
        if let Some(directory) = owned.iter().find(|directory| path.starts_with(directory)) {
            if output_part(path.strip_prefix(directory).expect("prefix")) {
                kinds.command_output += bytes;
            } else {
                kinds.conversations += bytes;
            }
        } else if programs.iter().any(|directory| path.starts_with(directory)) {
            kinds.program += bytes;
        } else if path.starts_with(&cache)
            && path != cache
            && path != viewed
            && path != viewed.join(".lock")
            && path != viewed.join(".generation")
            && !path.starts_with(viewed.join(".requests"))
        {
            kinds.cache += bytes;
        } else if path.strip_prefix(world.path().join("towns")).ok().is_some_and(|path| {
            let parts: Vec<_> = path.iter().collect();
            parts.len() >= 4 && parts[1] == "pages" && (parts[3] == "jobs" || parts[3] == "out")
        }) {
            kinds.command_output += bytes;
        } else {
            kinds.other += bytes;
        }
    })?;
    let everyone: HashMap<ResidentId, crate::residents::OnRecord> =
        node.residents.everyone().map(|resident| (resident.id.clone(), resident)).collect();
    let mut listed = Vec::new();
    for (name, one) in measured.iter_mut() {
        listed.push(describe(node, name, one, &everyone)?);
    }
    listed.sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.name.cmp(&b.name)));
    Ok(Space {
        machine: node.machine(),
        at: Timestamp::now(),
        root: crate::world::native(root),
        root_bytes,
        kinds,
        conversations: listed,
        output_days: crate::retention::days(world)?,
        claude_logins: claude_logins(&measured)?,
    })
}

/// Everything each conversation keeps here, by its directory's name.
pub(super) fn conversations(node: &Node) -> io::Result<BTreeMap<String, Measured>> {
    measure_conversations(node, None)
}

/// A deletion only depends on the records belonging to its own conversation.
pub(super) fn conversation(node: &Node, name: &str) -> io::Result<Option<Measured>> {
    Ok(measure_conversations(node, Some(name))?.remove(name))
}

fn measure_conversations(node: &Node, wanted: Option<&str>) -> io::Result<BTreeMap<String, Measured>> {
    let world = &node.world;
    let mut measured: BTreeMap<String, Measured> = BTreeMap::new();
    for kept in world.kept()? {
        if wanted.is_some_and(|wanted| kept.name != wanted) {
            continue;
        }
        let one = measured.entry(kept.name.clone()).or_default();
        one.places.push(kept.directory.path().to_path_buf());
        let aliases = match std::fs::read(kept.directory.aliases()) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
        if let Some(bytes) = aliases {
            let aliases: BTreeMap<PathBuf, PathBuf> = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
            for alias in aliases.into_keys() {
                match std::fs::symlink_metadata(&alias) {
                    Ok(_) => one.places.push(alias),
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error),
                }
            }
        }
        add_directory(one, kept.directory.path(), &node.log)?;
    }
    // What stays where the earlier layout put it belongs to the conversation
    // of the same name.
    for parent in world.earlier().parents()? {
        for directory in entries(&parent)? {
            let Some(name) = directory.file_name().and_then(|name| name.to_str()).map(str::to_string) else {
                continue;
            };
            if wanted.is_some_and(|wanted| name != wanted) || Kind::of(&name).is_none() || !directory.is_dir() {
                continue;
            }
            let one = measured.entry(name).or_default();
            if !one.places.contains(&directory) {
                one.places.push(directory.clone());
            }
            if !fastctx_platform::owned_directory::is_link(&std::fs::symlink_metadata(&directory)?) {
                add_directory(one, &directory, &node.log)?;
            }
        }
    }
    let claude = host::claude_sessions(world)?;
    let mut rollouts: HashMap<PathBuf, HashMap<String, Vec<PathBuf>>> = HashMap::new();
    let user_home = fastctx_platform::user_home()?;
    let environment = crate::claude::environment().map_err(|failure| io::Error::other(failure.message))?;
    let claude_default = crate::homes::claude_default(&environment, &user_home);

    for (name, one) in measured.iter_mut() {
        let Some(Kind::Home(id)) = Kind::of(name) else { continue };
        let directory = world.conversation(&id);
        // The plugin's directories for its Claude sessions are its own.
        for (session, exchange) in claude.get(&id).into_iter().flatten() {
            if !one.sessions.contains(session) {
                one.sessions.push(session.clone());
            }
            if !one.places.contains(exchange) {
                one.places.push(exchange.clone());
            }
            if !fastctx_platform::owned_directory::is_link(&std::fs::symlink_metadata(exchange)?) {
                one.records.add(tally(exchange)?);
            }
        }
        let about = match crate::residents::read_about(&directory) {
            Ok(about) => about,
            Err(error) if error.kind() == io::ErrorKind::NotFound => match crate::forks::retained(&directory)? {
                Some(about) => about,
                None => continue,
            },
            Err(_) => continue,
        };
        let mut host_record = HostRecord::None;
        let mut keeping_days = None;
        let mut transcript_present = false;
        // The host's own records of a session the user opened on their own
        // are theirs: the World neither counts nor reads them
        // (`world-layers.md`).
        let agent = if about.world_managed() { about.host.agent.as_str() } else { "" };
        match agent {
            "codex" => {
                if let Some(thread) = &about.session {
                    one.sessions.push(thread.clone());
                    let home = node
                        .codex
                        .of(node, &about)
                        .map_err(|error| io::Error::other(error.message))?
                        .session_home()
                        .to_path_buf();
                    if !rollouts.contains_key(&home) {
                        rollouts.insert(home.clone(), host::codex_rollouts(&home)?);
                    }
                    let files = rollouts[&home].get(thread).cloned().unwrap_or_default();
                    host_record = HostRecord::Missing { bytes: 0 };
                    for file in files {
                        one.host.add(tally(&file)?);
                        one.host_files.push(file);
                    }
                }
            }
            "claude" => {
                let home = about.home.clone().unwrap_or_else(|| claude_default.clone());
                let mut sessions: Vec<(String, Option<PathBuf>)> = claude
                    .get(&id)
                    .into_iter()
                    .flatten()
                    .map(|(session, exchange)| (session.clone(), Some(exchange.clone())))
                    .collect();
                if let Some(session) = &about.session
                    && !sessions.iter().any(|(known, _)| known == session)
                {
                    sessions.push((session.clone(), None));
                }
                let primary = sessions
                    .iter()
                    .find(|(session, exchange)| about.session.as_ref() == Some(session) && exchange.is_some())
                    .or_else(|| sessions.iter().find(|(_, exchange)| exchange.is_some()));
                let home = host::claude_home(&home, primary.and_then(|(_, exchange)| exchange.as_deref()))?;
                one.claude_home = Some(home.clone());
                keeping_days = host::claude_keeping(&home, crate::homes::same_dir(&home, &claude_default)).days;
                let mut said = false;
                let own = host::claude_own_project(&home, directory.path())?;
                for (session, exchange) in &sessions {
                    let (files, pointed) = host::claude_session_files(&home, session, exchange.as_deref())?;
                    said |= pointed;
                    for file in files {
                        transcript_present |= file.extension().is_some_and(|extension| extension == "jsonl");
                        if own.as_deref().is_some_and(|own| crate::homes::within(own, &file)) {
                            continue;
                        }
                        one.host.add(tally(&file)?);
                        one.host_files.push(file);
                    }
                }
                if let Some(own) = own {
                    one.host.add(tally(&own)?);
                    one.host_files.push(own);
                }
                if !sessions.is_empty() {
                    host_record = HostRecord::Missing { bytes: one.host.bytes };
                }
                if !transcript_present && said {
                    // Claude Code deletes a session untouched for its days;
                    // one active since cannot have gone that way.
                    let keeping = host::claude_keeping(&home, about.home.is_none());
                    let last = Some(crate::residents::read_now(&directory)?.map_or(about.born_at, |now| now.since));
                    let aged = keeping.days.is_some_and(|days| {
                        last.is_none_or(|last| {
                            Timestamp::now().as_second() - last.as_second() >= i64::from(days) * 24 * 60 * 60
                        })
                    });
                    if aged {
                        host_record = HostRecord::GoneByClaude { days: keeping.days, bytes: one.host.bytes };
                    }
                }
            }
            _ => {}
        }
        if !one.host_files.is_empty() && (about.host.agent.as_str() != "claude" || transcript_present) {
            host_record = HostRecord::Kept { bytes: one.host.bytes, days: keeping_days };
        }
        one.host_record = host_record;
    }
    for one in measured.values_mut() {
        let activity = one.records.latest;
        one.records = Size::default();
        one.output = Size::default();
        fastctx_platform::disk_usage::visit_many(one.places.iter().map(PathBuf::as_path), |path, metadata, bytes| {
            let size = Size { bytes, latest: activity_time(path, metadata) };
            let directory = one.places.iter().find(|directory| path.starts_with(directory)).expect("a visited place");
            if output_part(path.strip_prefix(directory).expect("prefix")) {
                one.output.add(size);
            } else {
                one.records.add(size);
            }
        })?;
        one.records.latest = one.records.latest.max(activity);
    }
    Ok(measured)
}

/// Adds what one directory of a conversation holds: its jobs and their
/// output apart from the rest.
fn add_directory(one: &mut Measured, directory: &Path, log: &crate::log::Log) -> io::Result<()> {
    fastctx_platform::disk_usage::visit(directory, |path, metadata, bytes| {
        let size = Size { bytes, latest: activity_time(path, metadata) };
        if output_part(path.strip_prefix(directory).expect("prefix")) {
            one.output.add(size);
        } else {
            one.records.add(size);
        }
    })?;
    #[derive(serde::Deserialize)]
    struct Happened {
        at: Option<Timestamp>,
    }
    for name in ["actions", "messages"] {
        let file = match std::fs::File::open(directory.join(name)) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        for happened in serde_json::Deserializer::from_reader(io::BufReader::new(file)).into_iter::<Happened>() {
            match happened {
                Ok(happened) => one.records.latest = one.records.latest.max(happened.at),
                Err(error) => {
                    log.line(format_args!(
                        "activity timestamp unreadable in {}, line {}, column {}",
                        directory.join(name).display(),
                        error.line(),
                        error.column()
                    ));
                    break;
                }
            }
        }
    }
    for entry in entries(directory)? {
        let name = entry.file_name().and_then(|name| name.to_str()).unwrap_or_default().to_string();
        if name == "legacy" {
            for legacy in entries(&entry)? {
                add_jobs(one, &legacy.join("jobs"))?;
            }
        }
        if name == "jobs" {
            add_jobs(one, &entry)?;
        }
    }
    Ok(())
}

fn activity_time(path: &Path, metadata: &std::fs::Metadata) -> Option<Timestamp> {
    let name = path.file_name()?.to_str()?;
    if metadata.is_dir()
        || fastctx_platform::owned_directory::is_link(metadata)
        || [
            "aliases.json",
            ".plan.json",
            "town",
            "actions",
            "messages",
            "seen",
            "books-retired",
            "host-deleted",
            "removed",
        ]
        .contains(&name)
    {
        return None;
    }
    metadata.modified().ok().and_then(|at| Timestamp::try_from(at).ok())
}

fn output_part(relative: &Path) -> bool {
    let parts: Vec<_> = relative.iter().collect();
    let first = if parts.first().is_some_and(|part| *part == "legacy") { parts.get(2) } else { parts.first() };
    first.is_some_and(|part| *part == "jobs" || *part == "out" || *part == "artifacts-delivered.json")
}

fn add_jobs(one: &mut Measured, jobs: &Path) -> io::Result<()> {
    for record in entries(jobs)? {
        if record.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        let job = match fastctx_hands::run::record::read(&record) {
            Ok(job) => job,
            Err(error) if error.kind() == io::ErrorKind::InvalidData => {
                one.unreadable_jobs.push(format!(
                    "cannot determine whether a job is running from {}: {error}; its conversation cannot be deleted",
                    record.display()
                ));
                continue;
            }
            Err(error) => return Err(error),
        };
        if job.end.is_none() || fastctx_hands::run::program::in_use(&job)? {
            one.running.push(job.id.to_string());
        }
        if one.job_town.as_ref().is_none_or(|(at, _)| job.started_at > *at) {
            one.job_town = Some((job.started_at, job.town.clone()));
        }
    }
    Ok(())
}

fn describe(
    node: &Node,
    name: &str,
    one: &mut Measured,
    everyone: &HashMap<ResidentId, crate::residents::OnRecord>,
) -> io::Result<ConversationSpace> {
    let kind = Kind::of(name).expect("only conversation directories are measured");
    let directory = ConversationDir::at(&node.world.conversations().join(name));
    let mut last = one.records.latest.max(one.output.latest).max(one.host.latest);
    let running = one.unreadable_jobs.first().cloned().or_else(|| {
        one.running
            .first()
            .map(|job| format!("{job}, a command it started, still runs here; stop it first (fastctx stop {job})"))
    });
    let (group, agent, title, town, lives_on, unopened, held) = match &kind {
        Kind::Home(id) => match everyone.get(id) {
            Some(resident) => {
                let now = resident.now.as_ref();
                last = Some(now.map_or(resident.about.born_at, |now| now.since))
                    .max(one.host.latest)
                    .max(one.job_town.as_ref().map(|(at, _)| *at));
                let busy = now.is_some_and(|now| now.activity == Activity::Busy || now.turn.is_some());
                let held = if busy {
                    Some("it is working; stop it first".to_string())
                } else if resident.here && !resident.about.world_managed() {
                    Some(format!("it is open in {}; close it there first", host_name(&resident.about.host.agent)))
                } else {
                    running
                };
                let group = if resident.removed.is_some() { Group::Removed } else { Group::Listed };
                let title = now.and_then(|now| now.title.clone());
                (
                    group,
                    resident.about.host.agent.clone(),
                    title,
                    resident.town.clone(),
                    Some(node.machine()),
                    false,
                    held,
                )
            }
            // Files attached for a conversation never opened.
            None => {
                if directory.about().exists() {
                    let about = crate::residents::read_about(&directory)?;
                    let now = crate::residents::read_now(&directory)?;
                    let places = crate::residents::read_places(&directory, &about)?;
                    return Ok(ConversationSpace {
                        name: name.to_string(),
                        group: Group::Removed,
                        agent: about.host.agent,
                        title: now.as_ref().and_then(|now| now.title.clone()),
                        town: places.now().cloned(),
                        lives_on: Some(about.machine),
                        last_activity: now.map(|now| now.since),
                        bytes: one.records.bytes + one.output.bytes + one.host_record.bytes(),
                        parts: Parts {
                            records: one.records.bytes,
                            command_output: one.output.bytes,
                            host: std::mem::take(&mut one.host_record),
                        },
                        unopened: false,
                        held: running,
                    });
                }
                let town = std::fs::read(directory.draft())
                    .ok()
                    .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
                    .and_then(|draft| draft.get("town")?.as_str()?.parse().ok());
                let held = if directory.draft_lock().try_exists()?
                    && fastctx_platform::FileLock::try_acquire(&directory.draft_lock())?.is_none()
                {
                    Some("it is being opened; wait for it to finish".into())
                } else {
                    running
                };
                (Group::Listed, id.agent(), None, town, Some(node.machine()), true, held)
            }
        },
        Kind::Guest(owner) => {
            let town = one.job_town.take().and_then(|(_, town)| town);
            (Group::Elsewhere, owner.resident.agent(), None, town, Some(owner.machine.clone()), false, running)
        }
        // Its room does not name the machine it lives on, and neither does this.
        Kind::Visiting { resident, .. } => {
            let town = one.job_town.take().and_then(|(_, town)| town);
            (Group::Elsewhere, resident.agent(), None, town, None, false, running)
        }
        Kind::Unplaced(id) => (Group::Elsewhere, id.agent(), None, None, None::<MachineName>, false, running),
    };
    let host = std::mem::take(&mut one.host_record);
    Ok(ConversationSpace {
        name: name.to_string(),
        group,
        agent,
        title,
        town,
        lives_on,
        last_activity: last,
        bytes: one.records.bytes + one.output.bytes + host.bytes(),
        parts: Parts { records: one.records.bytes, command_output: one.output.bytes, host },
        unopened,
        held,
    })
}

fn host_name(agent: &AgentName) -> &'static str {
    match agent.as_str() {
        "codex" => "Codex",
        "claude" => "Claude Code",
        _ => "its agent's window",
    }
}

/// How long each Claude Code login with a conversation here, and the
/// default one, keeps its sessions.
fn claude_logins(measured: &BTreeMap<String, Measured>) -> io::Result<Vec<ClaudeKeeping>> {
    let user_home = fastctx_platform::user_home()?;
    let environment = crate::claude::environment().map_err(|failure| io::Error::other(failure.message))?;
    let default = crate::homes::claude_default(&environment, &user_home);
    let mut logins: Vec<(PathBuf, bool)> = Vec::new();
    if default.is_dir() {
        logins.push((default.clone(), true));
    }
    for one in measured.values() {
        if let Some(home) = &one.claude_home
            && !logins.iter().any(|(known, _)| crate::homes::same_dir(known, home))
        {
            logins.push((home.clone(), false));
        }
    }
    Ok(logins.iter().map(|(home, default)| host::claude_keeping(home, *default)).collect())
}

/// The bytes under `path`, not following links, and when its newest file
/// changed. Nothing there is nothing.
pub(super) fn tally(path: &Path) -> io::Result<Size> {
    let mut size = Size::default();
    fastctx_platform::disk_usage::visit(path, |_, metadata, bytes| {
        size.bytes += bytes;
        if !metadata.is_dir() {
            size.latest = size.latest.max(metadata.modified().ok().and_then(|at| Timestamp::try_from(at).ok()));
        }
    })?;
    Ok(size)
}

pub(super) fn entries(path: &Path) -> io::Result<Vec<PathBuf>> {
    match std::fs::read_dir(path) {
        Ok(entries) => entries.map(|entry| entry.map(|entry| entry.path())).collect(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error),
    }
}
