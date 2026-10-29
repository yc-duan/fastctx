//! Where the World keeps things on one machine (`world-town.md`).
//!
//! Everything lives under `<fastctx root>/world/`. Every conversation this
//! machine keeps anything of has exactly one directory under
//! `conversations/`: its home, named by its id (`codex-7f3a`), when it lives
//! here; a guest room, named by its id and a tag of the machine it lives on
//! (`codex-7f3a_3e5f0c1d9a2b`), when it lives on another machine and worked
//! here. A guest room does not name that machine: what the conversation does
//! here, and whoever it works for here, sees its paths, and the machine it
//! lives on is not theirs to know (`world-run.md`). Rooms made before were
//! named by its address (`codex-7f3a@laptop`) and keep that name. The
//! directory does not depend on the conversation's project, so changing
//! projects moves nothing.
//!
//! Inside it, the conversation's own records and the originals of what the
//! user attached stay for as long as the conversation does; the commands it
//! ran here keep their records and output in `jobs/` and `out/`, which go
//! when nobody can read them any more; and its bookkeeping sits together in
//! `books/`, which goes once the conversation has ended and owes nothing.
//!
//! This module is the only place these paths are made. `earlier` knows the
//! layout before conversations had one directory each, for moving an older
//! World over and for the jobs an older program still runs there.

use std::path::{Path, PathBuf};

use crate::{Address, JobId, MachineName, ResidentId, TownName};

/// What names a machine, as the machine resolving a directory knows it
/// (`world-network.md`, 机器的名字): whether a name is its own, now or before
/// it was renamed, and every name another machine had, each of which still
/// names it in what was written under it.
pub trait Here {
    /// Whether `machine` names this machine.
    fn is_here(&self, machine: &MachineName) -> bool;

    /// Every name of the machine `machine` names, as known here; none when
    /// nothing more is known of it than the name itself.
    fn names_of(&self, machine: &MachineName) -> Vec<MachineName> {
        let _ = machine;
        Vec::new()
    }
}

/// A machine that knows only its name now.
impl Here for MachineName {
    fn is_here(&self, machine: &MachineName) -> bool {
        self == machine
    }
}

/// The names machines answer to, as one machine knows them.
#[derive(Clone, Debug)]
pub struct Names {
    /// This machine's name now.
    pub me: MachineName,
    /// The names this machine had, oldest first.
    pub was: Vec<MachineName>,
    /// Every other machine that was renamed, by all its names, its name now
    /// first.
    pub renamed: Vec<Vec<MachineName>>,
}

impl Here for Names {
    fn is_here(&self, machine: &MachineName) -> bool {
        self.me == *machine || self.was.contains(machine)
    }

    fn names_of(&self, machine: &MachineName) -> Vec<MachineName> {
        if self.is_here(machine) {
            return std::iter::once(self.me.clone()).chain(self.was.iter().cloned()).collect();
        }
        self.renamed.iter().find(|names| names.contains(machine)).cloned().unwrap_or_default()
    }
}

/// The World's directory on this machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorldDir {
    root: PathBuf,
}

impl WorldDir {
    /// The World kept under `fastctx_root`, the directory holding everything
    /// fastctx keeps for this user.
    pub fn under(fastctx_root: &Path) -> Self {
        Self { root: fastctx_root.join("world") }
    }

    /// The World kept in `world` itself.
    pub fn at(world: &Path) -> Self {
        Self { root: world.to_path_buf() }
    }

    pub fn path(&self) -> &Path {
        &self.root
    }

    /// Held by the node for as long as it runs; one node per World.
    pub fn node_lock(&self) -> PathBuf {
        self.root.join("node.lock")
    }

    /// Present while a system service keeps this World's node running: no
    /// one else starts a node for it then (`fastctx service node install`).
    pub fn node_service(&self) -> PathBuf {
        self.root.join("node.service")
    }

    /// The process of the node waiting to take this World from the one
    /// serving it, as it handed itself in: one goes before the next opens
    /// its endpoint, and a client that finds nobody listening then waits
    /// for that node while it lives.
    pub fn node_successor(&self) -> PathBuf {
        self.root.join("node.successor")
    }

    /// The node's own diagnostics.
    pub fn node_log(&self) -> PathBuf {
        self.root.join("node.log")
    }

    /// This machine's name.
    pub fn machine_file(&self) -> PathBuf {
        self.root.join("machine.json")
    }

    /// This machine's private key, made when it first joins a World.
    pub fn device_key(&self) -> PathBuf {
        self.root.join("device.key")
    }

    /// The key others seal what they send this machine to, made with the
    /// device key, or the first time a machine of an older World starts.
    pub fn seal_key(&self) -> PathBuf {
        self.root.join("seal.key")
    }

    /// The link to the hub: membership, letters, what was accepted.
    pub fn net(&self) -> PathBuf {
        self.root.join("net")
    }

    /// Which optional parts of the interface this device shows
    /// (`crate::features`).
    pub fn features(&self) -> PathBuf {
        self.root.join("features.json")
    }

    /// The networks this computer was on that were dissolved, until the
    /// user has seen so (`crate::accounts::Dissolution`).
    pub fn dissolved(&self) -> PathBuf {
        self.root.join("dissolved.json")
    }

    /// This computer's own model collaboration, which applies while it is
    /// in no network (`world-colleagues.md`, 同事从哪里来、能去哪里).
    pub fn own_collab(&self) -> PathBuf {
        self.root.join("collab.json")
    }

    /// How long command output stays after the model got its answer.
    pub fn retention(&self) -> PathBuf {
        self.root.join("retention.json")
    }

    /// Pictures read out of a host's record, unpacked while they are read:
    /// nothing in it outlives the node that read it.
    pub fn image_reading(&self) -> PathBuf {
        self.root.join("image-reading")
    }

    /// The directory holding every conversation's directory on this machine.
    pub fn conversations(&self) -> PathBuf {
        self.root.join("conversations")
    }

    /// What is remembered of the conversations deleted from this machine,
    /// one file each, for as long as something may still ask for them.
    pub fn deleted(&self) -> PathBuf {
        self.root.join("deleted")
    }

    /// Remote requesters' departure or return, one durable record each,
    /// independent of the resident cache rebuilt at startup.
    pub fn departed(&self) -> PathBuf {
        self.root.join("departed")
    }

    /// Conversation book directories plus the older shared queue, while an
    /// upgrade still has records whose owner cannot be recovered.
    pub fn book_directories(&self, name: &str) -> std::io::Result<Vec<PathBuf>> {
        let mut directories: Vec<_> = self.kept()?.into_iter().map(|kept| kept.directory.book(name)).collect();
        directories.push(self.root.join(name));
        Ok(directories)
    }

    /// The home of a conversation that lives on this machine.
    pub fn conversation(&self, id: &ResidentId) -> ConversationDir {
        ConversationDir { path: self.conversations().join(id.as_str()) }
    }

    /// The guest room of a conversation of another machine that worked
    /// here: its id with a tag of its machine, since a resident's id is only
    /// unique on the machine it lives on.
    pub fn guest(&self, owner: &Address) -> ConversationDir {
        ConversationDir { path: self.conversations().join(format!("{}_{}", owner.resident, room_tag(&owner.machine))) }
    }

    /// A guest room as rooms were named before they stopped naming the
    /// machine their conversation lives on.
    pub fn named_guest(&self, owner: &Address) -> ConversationDir {
        ConversationDir { path: self.conversations().join(owner.key()) }
    }

    /// The directory of `owner` on the machine `here`: its home or its guest
    /// room. A room made before keeps its name, and a machine renamed since
    /// its conversation first worked here keeps the room it had: what is
    /// kept there is found under any of its names.
    pub fn owner(&self, here: &impl Here, owner: &Address) -> ConversationDir {
        if here.is_here(&owner.machine) {
            return self.conversation(&owner.resident);
        }
        let mut machines = vec![owner.machine.clone()];
        machines.extend(here.names_of(&owner.machine).into_iter().filter(|machine| *machine != owner.machine));
        let of = |machine: &MachineName| Address { machine: machine.clone(), resident: owner.resident.clone() };
        machines
            .iter()
            .map(|machine| self.named_guest(&of(machine)))
            .chain(machines.iter().map(|machine| self.guest(&of(machine))))
            .find(|room| room.path.exists())
            .unwrap_or_else(|| self.guest(owner))
    }

    /// Where a job of `owner` keeps its record and output. Page work has its
    /// own directories in its project; no page is a conversation.
    pub fn job_owner(&self, here: &impl Here, owner: &crate::run::JobOwner) -> ConversationDir {
        match owner {
            crate::run::JobOwner::Resident(address) => self.owner(here, address),
            crate::run::JobOwner::Page { page } => {
                ConversationDir { path: self.town(&page.town).path().join("pages").join(page.page.as_str()) }
            }
        }
    }

    /// The record of `job` of `owner`, which a program of the layout before
    /// conversations had one directory each may still be running where that
    /// layout put it: that place, while it still holds the record; else the
    /// record's place now.
    pub fn job_record(
        &self,
        here: &impl Here,
        town: Option<&TownName>,
        owner: &crate::run::JobOwner,
        job: &JobId,
    ) -> PathBuf {
        let path = self.job_owner(here, owner).job_record(job);
        if let crate::run::JobOwner::Resident(address) = owner
            && !path.exists()
        {
            let earlier = self.earlier().owner(here, town, address).join("jobs").join(format!("{job}.json"));
            if earlier.exists() {
                return earlier;
            }
        }
        path
    }

    /// Every directory holding jobs of `owner` here: its conversation's,
    /// and those of the earlier layout that still hold jobs an older program
    /// started.
    pub fn job_directories(&self, here: &impl Here, owner: &Address) -> std::io::Result<Vec<PathBuf>> {
        let mut directories = vec![self.owner(here, owner).jobs()];
        let earlier = self.earlier();
        for parent in earlier.parents()? {
            let jobs = earlier.owner(here, earlier.town_of(&parent).as_ref(), owner).join("jobs");
            if jobs.is_dir() {
                directories.push(jobs);
            }
        }
        Ok(directories)
    }

    /// Parents of every directory that holds jobs: the conversations, each
    /// project's pages, and the places of the earlier layout that still hold
    /// jobs an older program started.
    pub fn job_parents(&self) -> std::io::Result<Vec<PathBuf>> {
        let mut parents = vec![self.conversations()];
        for kept in self.kept()? {
            let legacy = kept.directory.path().join("legacy");
            if legacy.is_dir() {
                parents.push(legacy);
            }
        }
        for town in self.town_directories()? {
            parents.push(town.join("pages"));
        }
        parents.extend(self.earlier().parents()?);
        Ok(parents)
    }

    /// Every conversation directory on this machine, homes and guest rooms.
    pub fn kept(&self) -> std::io::Result<Vec<Kept>> {
        let mut kept = Vec::new();
        for path in entries(&self.conversations())? {
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else { continue };
            if let Some(kind) = Kind::of(name)
                && path.is_dir()
            {
                kept.push(Kept { name: name.to_string(), kind, directory: ConversationDir { path } });
            }
        }
        kept.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(kept)
    }

    /// One town's directory on this machine.
    pub fn town(&self, town: &TownName) -> TownDir {
        TownDir { path: self.root.join("towns").join(town.as_str()) }
    }

    /// Every town this machine keeps a ledger for.
    pub fn towns(&self) -> std::io::Result<Vec<TownName>> {
        let mut towns = Vec::new();
        for path in self.town_directories()? {
            if let Some(town) = path.file_name().and_then(|name| TownName::try_from(name.to_str()?).ok())
                && (self.town(&town).signed().is_dir() || self.town(&town).path().join("index.json").is_file())
            {
                towns.push(town);
            }
        }
        towns.sort();
        Ok(towns)
    }

    fn town_directories(&self) -> std::io::Result<Vec<PathBuf>> {
        entries(&self.root.join("towns"))
    }

    /// Where a session the World starts stands, its host's working
    /// directory: its project's directory, or `outside`. It means nothing to
    /// the session, but groups a project's sessions in the hosts' own lists.
    pub fn standing(&self, town: Option<&TownName>) -> PathBuf {
        match town {
            Some(town) => self.town(town).path().to_path_buf(),
            None => self.root.join("outside"),
        }
    }

    /// The layout before every conversation had one directory.
    pub fn earlier(&self) -> Earlier<'_> {
        Earlier { world: self }
    }
}

/// The tag a guest room carries of the machine its conversation lives on:
/// the same for every room of that machine, and not its name.
pub fn room_tag(machine: &MachineName) -> String {
    use sha2::{Digest as _, Sha256};
    let digest = Sha256::new().chain_update(b"fastctx guest room\0").chain_update(machine.key().as_bytes()).finalize();
    digest.iter().take(ROOM_TAG / 2).map(|byte| format!("{byte:02x}")).collect()
}

/// How many hex digits a room tag has.
const ROOM_TAG: usize = 12;

/// What a conversation directory is, from its name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A conversation that lives on this machine.
    Home(ResidentId),
    /// A conversation of another machine that worked here, in a room made
    /// before rooms stopped naming that machine.
    Guest(Address),
    /// A conversation of another machine that worked here: `home` is the
    /// tag of that machine (`room_tag`).
    Visiting { resident: ResidentId, home: String },
    /// Copies of files attached in a conversation of another machine, from
    /// before the machine they came from was recorded: `<id>@~`.
    Unplaced(ResidentId),
}

/// The name an older World's copies of unknown origin are kept under.
pub const UNKNOWN_MACHINE: &str = "~";

impl Kind {
    /// The kind a directory named `name` holds; `None` for any other name,
    /// such as a directory being moved in, which starts with `.`.
    pub fn of(name: &str) -> Option<Self> {
        if let Some((resident, home)) = name.split_once('_') {
            let tag = home.len() == ROOM_TAG && home.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
            return tag
                .then(|| ResidentId::try_from(resident).ok())
                .flatten()
                .map(|resident| Self::Visiting { resident, home: home.to_string() });
        }
        match name.split_once('@') {
            Some((resident, UNKNOWN_MACHINE)) => ResidentId::try_from(resident).ok().map(Self::Unplaced),
            Some(_) => name.parse().ok().map(Self::Guest),
            None => ResidentId::try_from(name).ok().map(Self::Home),
        }
    }

    pub fn resident(&self) -> &ResidentId {
        match self {
            Self::Home(id) | Self::Unplaced(id) | Self::Visiting { resident: id, .. } => id,
            Self::Guest(address) => &address.resident,
        }
    }

    /// Whether this is a guest room of the conversation `resident` of the
    /// machine `machine` names, under that name or its tag.
    pub fn is_room_of(&self, resident: &ResidentId, machine: &MachineName) -> bool {
        match self {
            Self::Guest(owner) => owner.resident == *resident && owner.machine == *machine,
            Self::Visiting { resident: id, home } => id == resident && *home == room_tag(machine),
            Self::Home(_) | Self::Unplaced(_) => false,
        }
    }
}

/// One conversation directory on this machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Kept {
    pub name: String,
    pub kind: Kind,
    pub directory: ConversationDir,
}

/// One town's directory on one machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TownDir {
    path: PathBuf,
}

impl TownDir {
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The town's shared ledger as files: the versions of its record, its
    /// journals.
    pub fn ledger(&self) -> PathBuf {
        self.path.join("ledger")
    }

    /// Readable, read-only project-memory projections.
    pub fn journals(&self) -> PathBuf {
        self.path.join("journals")
    }

    pub fn specs(&self) -> PathBuf {
        self.path.join("specs")
    }

    /// Each entry of the ledger as its author signed it, named by its
    /// number.
    pub fn signed(&self) -> PathBuf {
        self.path.join("signed")
    }

    /// Attachments too large for a ledger entry, kept on the machine whose
    /// entry names them: one directory per entry.
    pub fn attachments(&self) -> PathBuf {
        self.path.join("attachments")
    }
}

/// One conversation's directory on one machine, or a page's work.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConversationDir {
    path: PathBuf,
}

impl ConversationDir {
    pub fn claude_exchange(&self, session: &str) -> PathBuf {
        self.path.join("claude").join(session)
    }

    pub fn aliases(&self) -> PathBuf {
        self.path.join("aliases.json")
    }
    /// The directory itself, as a place with no other meaning.
    pub fn at(path: &Path) -> Self {
        Self { path: path.to_path_buf() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Who the resident is and how it came to be.
    pub fn about(&self) -> PathBuf {
        self.path.join("about")
    }

    /// The project it is in now, and those it was in before (`Places`).
    pub fn town(&self) -> PathBuf {
        self.path.join("town")
    }

    /// What it is doing now, replaced whole when that changes.
    pub fn now(&self) -> PathBuf {
        self.path.join("now")
    }

    /// Present once the user removed it from the World: when.
    pub fn removed(&self) -> PathBuf {
        self.path.join("removed")
    }

    /// What the World saw the resident do, one JSON line each.
    pub fn actions(&self) -> PathBuf {
        self.path.join("actions")
    }

    /// A colleague's request, what it was told and how it answered, one
    /// JSON line each.
    pub fn messages(&self) -> PathBuf {
        self.path.join("messages")
    }

    /// What the user attached, as uploaded: one directory each.
    pub fn attachments(&self) -> PathBuf {
        self.path.join("attachments")
    }

    /// Present while files are attached for a conversation not opened yet.
    pub fn draft(&self) -> PathBuf {
        self.path.join("attachment-draft.json")
    }

    /// Held while a drafted conversation is being opened.
    pub fn draft_lock(&self) -> PathBuf {
        self.path.join("attachment-open.lock")
    }

    /// What each call of the World's tools did, as the history shows it.
    pub fn calls(&self) -> PathBuf {
        self.path.join("calls")
    }

    /// What only the World knows of the session: who said what, how turns
    /// ended, how the session fared.
    pub fn session_events(&self) -> PathBuf {
        self.path.join("session-events")
    }

    /// The order the next session event takes.
    pub fn session_sequence(&self) -> PathBuf {
        self.path.join("session-events-sequence.json")
    }

    /// How each turn ended, and who was owed its end.
    pub fn turns(&self) -> PathBuf {
        self.path.join("turns")
    }

    /// Words handed to the host together, and the turn that took them.
    pub fn handovers(&self) -> PathBuf {
        self.path.join("handovers")
    }

    /// Words whose taking by the host nobody knows.
    pub fn uncertain(&self) -> PathBuf {
        self.path.join("uncertain")
    }

    /// What is on its way into the resident's session, one file each
    /// (`Pending`).
    pub fn inbox(&self) -> PathBuf {
        self.path.join("inbox")
    }

    /// Ends of the resident's turns on their way to their readers, one file
    /// each (`Owing`).
    pub fn outbox(&self) -> PathBuf {
        self.path.join("outbox")
    }

    /// The shared draft and queue of what the user writes to it.
    pub fn composer(&self) -> PathBuf {
        self.path.join("composer.json")
    }

    /// The settings a Claude Code session the World started was started
    /// with; Claude Code reads it again whenever it starts it again.
    pub fn claude_settings(&self) -> PathBuf {
        self.path.join("claude-settings.json")
    }

    /// The instructions such a session was started with.
    pub fn claude_instructions(&self) -> PathBuf {
        self.path.join("claude-instructions.md")
    }

    pub fn jobs(&self) -> PathBuf {
        self.path.join("jobs")
    }

    pub fn out(&self) -> PathBuf {
        self.path.join("out")
    }

    /// When the latest end of a job here reached its reader: the clock of
    /// output left behind without a record.
    pub fn artifacts_delivered(&self) -> PathBuf {
        self.path.join("artifacts-delivered.json")
    }

    pub fn job_record(&self, job: &JobId) -> PathBuf {
        self.jobs().join(format!("{job}.json"))
    }

    /// Every byte a job wrote.
    pub fn job_output(&self, job: &JobId) -> PathBuf {
        self.out().join(format!("{job}.log"))
    }

    /// Working files retained across runs, owned by this conversation here.
    pub fn scratch(&self) -> PathBuf {
        self.path.join("scratch")
    }

    /// Temporary files owned by this job share its confirmed-end lifetime.
    pub fn job_scratch(&self, job: &JobId) -> PathBuf {
        self.out().join(format!("{job}.tmp"))
    }

    /// Present while the resident is owed word of the job's end: the job
    /// went on in the background after its call answered.
    pub fn job_owed(&self, job: &JobId) -> PathBuf {
        self.jobs().join(format!("{job}.owed"))
    }

    /// The script a job runs, as the shell reads it.
    pub fn job_script(&self, job: &JobId) -> PathBuf {
        self.jobs().join(format!("{job}.sh"))
    }

    /// The conversation's bookkeeping, all of it: what keeps it from being
    /// told the same thing twice, the settings of its host session, what it
    /// saw of files. It goes once the conversation has ended and owes nothing.
    pub fn books(&self) -> PathBuf {
        self.path.join("books")
    }

    pub fn book(&self, name: &str) -> PathBuf {
        self.books().join(name)
    }

    /// The ids of everything that went into the resident's session, one a
    /// line, as older nodes kept them.
    pub fn delivered(&self) -> PathBuf {
        self.books().join("delivered")
    }

    /// One empty file for each thing that went into the resident's session:
    /// the same thing arriving again is known and not told twice.
    pub fn delivery_receipts(&self) -> PathBuf {
        self.books().join("delivery-receipts")
    }

    /// One file for each turn's end that reached its reader's machine.
    pub fn paid(&self) -> PathBuf {
        self.books().join("paid")
    }

    /// Which host session each turn ran in.
    pub fn turn_sessions(&self) -> PathBuf {
        self.books().join("turn-sessions")
    }

    /// What the shared composer did for each operation, of one `kind`:
    /// `receipts`, `ended`, `uploads`.
    pub fn composer_books(&self, kind: &str) -> PathBuf {
        self.books().join(format!("composer-{kind}"))
    }

    /// What the resident saw of files here, one JSON line each.
    pub fn seen(&self) -> PathBuf {
        self.books().join("seen")
    }

    /// Jobs its runs handed to other machines left running there.
    pub fn elsewhere(&self) -> PathBuf {
        self.books().join("elsewhere")
    }

    /// What the model is told of the machines and folders the user points
    /// the conversation to, as of the last time they were set.
    pub fn focus(&self) -> PathBuf {
        self.books().join("focus.md")
    }

    /// The machines and folders a Codex thread's instructions were written
    /// with, and what those instructions say of them (`focus::Opening`).
    pub fn focus_opening(&self) -> PathBuf {
        self.books().join("focus-opening.json")
    }

    /// The model and reasoning the user chose when the session was opened.
    pub fn session_options(&self) -> PathBuf {
        self.books().join("options.json")
    }

    /// The model and reasoning chosen for the session's next turns.
    pub fn choices(&self) -> PathBuf {
        self.books().join("choices.json")
    }

    /// How a colleague was configured when it was asked for.
    pub fn callee(&self) -> PathBuf {
        self.books().join("callee.json")
    }

    /// The account each colleague it asked for works for, by machine.
    pub fn asked(&self) -> PathBuf {
        self.books().join("asked")
    }

    /// What a Claude turn that runs now has used so far.
    pub fn turn_facts(&self) -> PathBuf {
        self.books().join("turn-facts")
    }
}

/// The layout before every conversation had one directory: a resident's
/// records under the town it lived in, `towns/<town>/residents/<id>`, or
/// `outside/residents/<id>`, one such directory in each town it lived in;
/// what a resident of another machine did here under `<id>@<machine>`
/// beside them; and some of its records under the World's own directories.
pub struct Earlier<'a> {
    world: &'a WorldDir,
}

impl Earlier<'_> {
    /// Every directory of the earlier layout that holds residents'
    /// directories and is still there.
    pub fn parents(&self) -> std::io::Result<Vec<PathBuf>> {
        let mut parents = Vec::new();
        let outside = self.world.root.join("outside").join("residents");
        if outside.is_dir() {
            parents.push(outside);
        }
        for town in self.world.town_directories()? {
            let residents = town.join("residents");
            if residents.is_dir() {
                parents.push(residents);
            }
        }
        Ok(parents)
    }

    /// The town a parent of the earlier layout belongs to, from where it
    /// lies; `None` for `outside`.
    pub fn town_of(&self, parent: &Path) -> Option<TownName> {
        let town = parent.parent()?.strip_prefix(self.world.root.join("towns")).ok()?;
        TownName::try_from(town.to_str()?).ok()
    }

    /// The directory a resident named `name` had in `town`.
    pub fn directory(&self, town: Option<&TownName>, name: &str) -> PathBuf {
        let base = match town {
            Some(town) => self.world.root.join("towns").join(town.as_str()),
            None => self.world.root.join("outside"),
        };
        base.join("residents").join(name)
    }

    /// The directory the jobs of `owner` were put in when it lived in `town`.
    pub fn owner(&self, here: &impl Here, town: Option<&TownName>, owner: &Address) -> PathBuf {
        let name = if here.is_here(&owner.machine) { owner.resident.to_string() } else { owner.key() };
        self.directory(town, &name)
    }

    /// Where a removed resident's removal was recorded.
    pub fn removed(&self) -> PathBuf {
        self.world.root.join("removed-residents")
    }

    /// Where the earlier layout kept a Codex resident's session options,
    /// colleague configuration, colleagues' accounts and model choices.
    pub fn codex(&self) -> PathBuf {
        self.world.root.join("codex")
    }

    /// Where runs left running on another machine were recorded, by resident.
    pub fn elsewhere(&self) -> PathBuf {
        self.world.root.join("elsewhere")
    }
}

fn entries(path: &Path) -> std::io::Result<Vec<PathBuf>> {
    match std::fs::read_dir(path) {
        Ok(entries) => entries.map(|entry| entry.map(|entry| entry.path())).collect(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error),
    }
}
