//! This machine's name in the World.
//!
//! A machine that has not joined a World with others is a World of one
//! machine. Its name is chosen the first time its node starts, from the
//! host name, and kept in `machine.json`; joining a World later may rename
//! it, and so may the hubs it is on (`world-network.md`, 机器的名字). The
//! names it had are kept beside its name: what was written under one, a
//! conversation's records, a job's, a letter, is still this machine's.

use std::io;

use fastctx_platform::{Permissions, hostname, publish};
use fastctx_types::MachineName;
use fastctx_types::layout::WorldDir;
use serde::{Deserialize, Serialize};

/// This machine's name, and the names it had, oldest first.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MachineFile {
    pub name: MachineName,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub was: Vec<MachineName>,
}

impl MachineFile {
    /// Whether `machine` names this machine, now or before.
    pub fn is(&self, machine: &MachineName) -> bool {
        self.name == *machine || self.was.contains(machine)
    }
}

/// The machine's name and the names it had, chosen and recorded on first
/// use.
pub fn names(world: &WorldDir) -> io::Result<MachineFile> {
    let path = world.machine_file();
    match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, format!("{}: {error}", path.display()))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let file = MachineFile { name: MachineName::from_host(&hostname()?), was: Vec::new() };
            write(world, &file)?;
            Ok(file)
        }
        Err(error) => Err(error),
    }
}

/// Records `name` as this machine's name, as when it joins a World under it
/// or a hub renames it. Returns what is recorded.
pub fn rename(world: &WorldDir, name: &MachineName) -> io::Result<MachineFile> {
    let file = renamed(&names(world)?, name);
    keep(world, &file)?;
    Ok(file)
}

/// `file` with `name` as the machine's name: the name it had joins those it
/// had before, and one it had before that it takes again leaves them.
pub fn renamed(file: &MachineFile, name: &MachineName) -> MachineFile {
    let mut file = file.clone();
    let old = std::mem::replace(&mut file.name, name.clone());
    file.was.retain(|was| was != name);
    if old != *name && !file.was.contains(&old) {
        file.was.push(old);
    }
    file
}

/// Keeps `file` as `machine.json`.
pub fn keep(world: &WorldDir, file: &MachineFile) -> io::Result<()> {
    write(world, file)
}

fn write(world: &WorldDir, file: &MachineFile) -> io::Result<()> {
    let mut bytes = serde_json::to_vec_pretty(file).map_err(io::Error::other)?;
    bytes.push(b'\n');
    publish(&world.machine_file(), &bytes, Permissions::Private)
}
