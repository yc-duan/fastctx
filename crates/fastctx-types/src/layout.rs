//! Where the World keeps things on one machine (`world-town.md`).
//!
//! Everything lives under `<fastctx root>/world/`. A resident's records sit
//! in its town's directory, or under `outside/` before it enters one; the
//! jobs it starts and their output sit beside them on the machine where
//! they run.

use std::path::{Path, PathBuf};

use crate::{Address, JobId, MachineName, ResidentId, TownName};

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

    /// This computer's own model collaboration, which applies while it is
    /// in no network (`world-colleagues.md`, 同事从哪里来、能去哪里).
    pub fn own_collab(&self) -> PathBuf {
        self.root.join("collab.json")
    }

    /// Every directory that holds residents' directories: `outside` and
    /// each town's.
    pub fn resident_parents(&self) -> std::io::Result<Vec<PathBuf>> {
        let mut parents = vec![self.root.join("outside").join("residents")];
        match std::fs::read_dir(self.root.join("towns")) {
            Ok(towns) => {
                for town in towns {
                    parents.push(town?.path().join("residents"));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        Ok(parents)
    }

    /// One town's directory on this machine.
    pub fn town(&self, town: &TownName) -> TownDir {
        TownDir { path: self.root.join("towns").join(town.as_str()) }
    }

    /// Every town this machine keeps a ledger for.
    pub fn towns(&self) -> std::io::Result<Vec<TownName>> {
        let mut towns = Vec::new();
        match std::fs::read_dir(self.root.join("towns")) {
            Ok(entries) => {
                for entry in entries {
                    let path = entry?.path();
                    if let Some(town) = path.file_name().and_then(|name| TownName::try_from(name.to_str()?).ok())
                        && self.town(&town).signed().is_dir()
                    {
                        towns.push(town);
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        towns.sort();
        Ok(towns)
    }

    /// The directory of a resident that lives on this machine: in its town,
    /// or outside every town.
    pub fn resident(&self, town: Option<&TownName>, resident: &ResidentId) -> ResidentDir {
        let base = self.residents(town).join(resident.as_str());
        let path = if self.removed(resident).exists() { base.join("archive") } else { base };
        ResidentDir { path }
    }

    /// A durable membership tombstone, independent of the resident's town.
    pub fn removed(&self, resident: &ResidentId) -> PathBuf {
        self.root.join("removed-residents").join(format!("{resident}.json"))
    }

    /// The directory of what a resident of another machine did here, such
    /// as the jobs it ran: named with its machine, since a resident's id is
    /// only unique on the machine it lives on.
    pub fn visitor(&self, town: Option<&TownName>, owner: &Address) -> ResidentDir {
        ResidentDir { path: self.residents(town).join(owner.tag()) }
    }

    /// The directory for `owner` on the machine `here`.
    pub fn owner(&self, here: &MachineName, town: Option<&TownName>, owner: &Address) -> ResidentDir {
        if owner.machine == *here { self.resident(town, &owner.resident) } else { self.visitor(town, owner) }
    }

    /// Page work has its own directories; no page is registered as a resident.
    pub fn job_owner(&self, here: &MachineName, town: Option<&TownName>, owner: &crate::run::JobOwner) -> ResidentDir {
        match owner {
            crate::run::JobOwner::Resident(address) => self.owner(here, town, address),
            crate::run::JobOwner::Page { page } => {
                ResidentDir { path: self.town(&page.town).path().join("pages").join(page.page.as_str()) }
            }
        }
    }

    /// Parents of all job owners, including pages as well as residents.
    pub fn job_parents(&self) -> std::io::Result<Vec<PathBuf>> {
        let mut parents = self.resident_parents()?;
        let pages: Vec<_> = parents
            .iter()
            .map(|parent| parent.parent().expect("a resident parent belongs to a place").join("pages"))
            .collect();
        parents.extend(pages);
        Ok(parents)
    }

    fn residents(&self, town: Option<&TownName>) -> PathBuf {
        let base = match town {
            Some(town) => self.root.join("towns").join(town.as_str()),
            None => self.root.join("outside"),
        };
        base.join("residents")
    }
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

/// One resident's directory on one machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResidentDir {
    path: PathBuf,
}

impl ResidentDir {
    pub fn calls(&self) -> PathBuf {
        self.path.join("calls")
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Who the resident is and how it came to be.
    pub fn about(&self) -> PathBuf {
        self.path.join("about")
    }

    /// What it is doing now, replaced whole when that changes.
    pub fn now(&self) -> PathBuf {
        self.path.join("now")
    }

    /// Present once the resident moved to another town (`Moved`).
    pub fn moved(&self) -> PathBuf {
        self.path.join("moved")
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

    /// What the resident saw of files, one JSON line each.
    pub fn seen(&self) -> PathBuf {
        self.path.join("seen")
    }

    /// What is on its way into the resident's session, one file each
    /// (`Pending`).
    pub fn inbox(&self) -> PathBuf {
        self.path.join("inbox")
    }

    /// The ids of everything that went into the resident's session, one a
    /// line: the same thing arriving again is known and not told twice.
    pub fn delivered(&self) -> PathBuf {
        self.path.join("delivered")
    }

    /// Ends of the resident's turns on their way to their readers, one file
    /// each (`Owing`).
    pub fn outbox(&self) -> PathBuf {
        self.path.join("outbox")
    }

    pub fn jobs(&self) -> PathBuf {
        self.work_path().join("jobs")
    }

    pub fn out(&self) -> PathBuf {
        self.work_path().join("out")
    }

    fn work_path(&self) -> &Path {
        if self.path.file_name().is_some_and(|name| name == "archive") {
            self.path.parent().expect("archive is inside a resident directory")
        } else {
            &self.path
        }
    }

    pub fn job_record(&self, job: &JobId) -> PathBuf {
        self.jobs().join(format!("{job}.json"))
    }

    /// Every byte a job wrote.
    pub fn job_output(&self, job: &JobId) -> PathBuf {
        self.out().join(format!("{job}.log"))
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
}
