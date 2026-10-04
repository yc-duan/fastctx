//! Where the World keeps things on one machine (`world-town.md`).
//!
//! Everything lives under `<fastctx root>/world/`. A resident's records sit
//! in its town's directory, or under `outside/` before it enters one; the
//! jobs it starts and their output sit beside them on the machine where
//! they run.

use std::path::{Path, PathBuf};

use crate::{JobId, ResidentId, TownName};

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

    /// The node's own diagnostics.
    pub fn node_log(&self) -> PathBuf {
        self.root.join("node.log")
    }

    /// This machine's settings: its name, and later its membership.
    pub fn machine_file(&self) -> PathBuf {
        self.root.join("machine.json")
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

    /// A resident's directory: in its town, or outside every town.
    pub fn resident(&self, town: Option<&TownName>, resident: &ResidentId) -> ResidentDir {
        let base = match town {
            Some(town) => self.root.join("towns").join(town.as_str()),
            None => self.root.join("outside"),
        };
        ResidentDir { path: base.join("residents").join(resident.as_str()) }
    }
}

/// One resident's directory on one machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResidentDir {
    path: PathBuf,
}

impl ResidentDir {
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Who the resident is and how it came to be.
    pub fn about(&self) -> PathBuf {
        self.path.join("about")
    }

    /// What the World saw the resident do, one JSON line each.
    pub fn actions(&self) -> PathBuf {
        self.path.join("actions")
    }

    /// What the resident saw of files, one JSON line each.
    pub fn seen(&self) -> PathBuf {
        self.path.join("seen")
    }

    /// What waits to be told to the resident, one file each.
    pub fn inbox(&self) -> PathBuf {
        self.path.join("inbox")
    }

    pub fn jobs(&self) -> PathBuf {
        self.path.join("jobs")
    }

    pub fn out(&self) -> PathBuf {
        self.path.join("out")
    }

    pub fn job_record(&self, job: &JobId) -> PathBuf {
        self.jobs().join(format!("{job}.json"))
    }

    /// Every byte a job wrote.
    pub fn job_output(&self, job: &JobId) -> PathBuf {
        self.out().join(format!("{job}.log"))
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
