//! This machine's name in the World.
//!
//! A machine that has not joined a World with others is a World of one
//! machine. Its name is chosen the first time its node starts, from the
//! host name, and kept in `machine.json`; joining a World later may rename
//! it.

use std::io;

use fastctx_platform::{Permissions, hostname, publish};
use fastctx_types::MachineName;
use fastctx_types::layout::WorldDir;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct MachineFile {
    name: MachineName,
}

/// The machine's name, chosen and recorded on first use.
pub fn name(world: &WorldDir) -> io::Result<MachineName> {
    let path = world.machine_file();
    match std::fs::read(&path) {
        Ok(bytes) => {
            let file: MachineFile = serde_json::from_slice(&bytes)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, format!("{}: {error}", path.display())))?;
            Ok(file.name)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let name = MachineName::from_host(&hostname()?);
            let mut bytes = serde_json::to_vec_pretty(&MachineFile { name: name.clone() }).map_err(io::Error::other)?;
            bytes.push(b'\n');
            publish(&path, &bytes, Permissions::Private)?;
            Ok(name)
        }
        Err(error) => Err(error),
    }
}

/// Records `name` as this machine's name, as when it joins a World under it.
pub fn rename(world: &WorldDir, name: &MachineName) -> io::Result<()> {
    let mut bytes = serde_json::to_vec_pretty(&MachineFile { name: name.clone() }).map_err(io::Error::other)?;
    bytes.push(b'\n');
    publish(&world.machine_file(), &bytes, Permissions::Private)
}
