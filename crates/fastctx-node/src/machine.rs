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
            let name = from_hostname(&hostname()?);
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

/// A machine name made from a host name: lowercase, every run of other
/// characters turned into one hyphen, cut to the length a machine name
/// allows. A host name with no letters or digits becomes `machine`.
fn from_hostname(host: &str) -> MachineName {
    // A DNS host name may carry its domain; the first label names the host.
    let label = host.split('.').next().unwrap_or(host);
    let mut word = String::new();
    for c in label.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            word.push(c);
        } else if !word.is_empty() && !word.ends_with('-') {
            word.push('-');
        }
    }
    if word.trim_matches('-').is_empty() {
        word = "machine".to_string();
    } else if !word.starts_with(|c: char| c.is_ascii_lowercase()) {
        // A machine name starts with a letter.
        word.insert_str(0, "m-");
    }
    word.truncate(32);
    while word.ends_with('-') {
        word.pop();
    }
    // `hub` is the hub's own name; no machine may take it.
    MachineName::try_from(word.as_str())
        .or_else(|_| MachineName::try_from("machine"))
        .expect("`machine` is a valid machine name")
}
