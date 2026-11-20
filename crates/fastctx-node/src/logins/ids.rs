//! Each login's id for good (`LoginId`), kept in `<world>/login-ids.json`
//! apart from what a scan finds, which a scan builds anew. A login is the
//! directory it is written as, two spellings of one directory being one
//! login (`crate::homes::same_dir`); it is given an id the first time a
//! session the World starts is born in it, and the id is on disk before
//! the session is. An id once given stays the directory's: nothing is taken
//! out, and a record that cannot be read is never written over, so that no
//! login is ever given a second id.

use std::io;
use std::path::{Path, PathBuf};

use fastctx_platform::{Permissions, publish};
use fastctx_types::AgentName;
use fastctx_types::logins::LoginId;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::Node;

const VERSION: u32 = 1;
const RECORD_BYTES: u32 = 4 * 1024 * 1024;
const LOGINS: usize = 4096;

#[derive(Default, Serialize, Deserialize)]
struct Registry {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    logins: Vec<Registered>,
}

#[derive(Serialize, Deserialize)]
struct Registered {
    id: LoginId,
    agent: AgentName,
    /// The directory as it was written when it was given its id.
    dir: PathBuf,
    at: Timestamp,
}

fn path(world: &Path) -> PathBuf {
    world.join("login-ids.json")
}

fn read(world: &Path) -> io::Result<Registry> {
    let registry: Registry = match fastctx_platform::durable_file::read_bounded(&path(world), RECORD_BYTES) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, format!("login-ids.json: {error}")))?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Registry::default()),
        Err(error) => return Err(error),
    };
    if registry.version > VERSION {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("login-ids.json is of version {}, newer than this fastctx reads", registry.version),
        ));
    }
    Ok(registry)
}

/// The id of `agent`'s login `dir` on this machine, given and kept now
/// where it had none.
pub(crate) fn of(node: &Node, agent: &AgentName, dir: &Path) -> io::Result<LoginId> {
    let _giving = node.logins.giving_ids.lock().expect("never poisoned");
    given(node.world.path(), agent, dir)
}

fn given(world: &Path, agent: &AgentName, dir: &Path) -> io::Result<LoginId> {
    let mut registry = read(world)?;
    if let Some(known) =
        registry.logins.iter().find(|known| known.agent == *agent && crate::homes::same_dir(&known.dir, dir))
    {
        return Ok(known.id.clone());
    }
    if registry.logins.len() >= LOGINS {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "login-ids.json holds 4096 logins already"));
    }
    let id = loop {
        let mut bytes = [0_u8; 8];
        getrandom::fill(&mut bytes).map_err(io::Error::other)?;
        let id = LoginId::from_bytes(bytes);
        if !registry.logins.iter().any(|known| known.id == id) {
            break id;
        }
    };
    registry.logins.push(Registered {
        id: id.clone(),
        agent: agent.clone(),
        dir: dir.to_path_buf(),
        at: Timestamp::now(),
    });
    registry.version = VERSION;
    let mut bytes = serde_json::to_vec_pretty(&registry).map_err(io::Error::other)?;
    bytes.push(b'\n');
    publish(&path(world), &bytes, Permissions::Private)?;
    Ok(id)
}

/// The login a session of `agent` the World starts here is born in: `home`,
/// or the agent's default one as it stands now, as the World starts it
/// (`crate::homes::listed` writes the default one as none). None for an
/// agent that keeps no logins the World knows, or where the id could not
/// be read or kept, which the node's log tells: the session is born all
/// the same, as one born before ids were kept.
pub(crate) fn born(node: &Node, agent: &AgentName, home: Option<&Path>) -> Option<LoginId> {
    if !super::AGENTS.contains(&agent.as_str()) {
        return None;
    }
    let dir = match home {
        Some(home) => home.to_path_buf(),
        None => match crate::adoption::default_login(node, agent.as_str()) {
            Ok(dir) => dir,
            Err(error) => {
                node.log.line(format_args!("finding the default {agent} login for a session's login id: {error}"));
                return None;
            }
        },
    };
    match of(node, agent, &dir) {
        Ok(id) => Some(id),
        Err(error) => {
            node.log.line(format_args!("giving the {agent} login {} its id: {error}", dir.display()));
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(name: &str) -> AgentName {
        AgentName::try_from(name).expect("an agent's name")
    }

    #[test]
    fn a_login_keeps_the_id_it_was_given() {
        let world = tempfile::tempdir().expect("a directory");
        let home = world.path().join("claude-home");
        fastctx_platform::disk::create_dir_all(&home).expect("a login");
        let first = given(world.path(), &agent("claude"), &home).expect("an id");
        assert!(fastctx_platform::disk::is_file(path(world.path())), "kept before it is answered");
        assert_eq!(given(world.path(), &agent("claude"), &home).expect("an id"), first);
        // Another spelling of the same directory is the same login.
        let mut spelled = home.clone().into_os_string();
        spelled.push("/");
        assert_eq!(given(world.path(), &agent("claude"), Path::new(&spelled)).expect("an id"), first);
        // Another agent's login in that directory, or another directory, is another.
        let codex = given(world.path(), &agent("codex"), &home).expect("an id");
        assert_ne!(codex, first);
        let other = world.path().join("other-home");
        let second = given(world.path(), &agent("claude"), &other).expect("an id");
        assert_ne!(second, first);
        assert_ne!(second, codex);
        // Read again, as after a restart.
        assert_eq!(given(world.path(), &agent("claude"), &home).expect("an id"), first);
        assert_eq!(read(world.path()).expect("the record").logins.len(), 3);
    }

    #[test]
    fn a_record_that_cannot_be_read_is_never_written_over() {
        let world = tempfile::tempdir().expect("a directory");
        let home = world.path().join("codex-home");
        for written in [&b"{ not json"[..], &br#"{"version": 2, "logins": []}"#[..]] {
            fastctx_platform::disk::write(path(world.path()), written).expect("a record");
            assert!(given(world.path(), &agent("codex"), &home).is_err());
            assert_eq!(fastctx_platform::disk::read(path(world.path())).expect("the record"), written);
        }
    }
}
