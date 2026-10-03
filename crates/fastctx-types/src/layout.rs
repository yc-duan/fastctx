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

    /// The script a job runs, as the shell reads it.
    pub fn job_script(&self, job: &JobId) -> PathBuf {
        self.jobs().join(format!("{job}.sh"))
    }
}
