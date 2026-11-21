//! Each login's id for good (`LoginId`), kept in `<world>/login-ids.json`
//! apart from what a scan finds, which a scan builds anew. A login is the
//! directory a session is started in, as it really is on disk when the
//! login is given its id: however it is written, through whatever link, the
//! directory it resolves to then is the one the id is kept for, and is looked
//! up anew, never as remembered, each time an id is asked for. A directory
//! that cannot be resolved has no id (codex-d1962125). An id is given the
//! first time a session the World starts is started in the login, and is on
//! disk before the session records it. An id once given stays the
//! directory's: nothing is taken out, and a record that cannot be read, or
//! does not hold together, is never written over, so that no login is ever
//! given a second id. A login moved to another disk leaves a link where its
//! directory was (`fastctx login move`): the directory kept for its id then
//! resolves to where it went, and the id goes with it.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

use fastctx_platform::{Permissions, publish};
use fastctx_types::AgentName;
use fastctx_types::logins::LoginId;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::Node;

const VERSION: u32 = 1;
const RECORD_BYTES: usize = 4 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
struct Registry {
    version: u32,
    logins: Vec<Registered>,
}

#[derive(Serialize, Deserialize)]
struct Registered {
    id: LoginId,
    agent: AgentName,
    /// The directory as it resolved on disk when it was given its id.
    dir: PathBuf,
    /// As it was written then.
    written: PathBuf,
    at: Timestamp,
}

fn path(world: &Path) -> PathBuf {
    world.join("login-ids.json")
}

fn invalid(why: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("login-ids.json: {why}"))
}

/// The record as kept; none yet is an empty one. One that cannot be read,
/// is of a newer version, or gives an id or a directory twice, is an error.
fn read(world: &Path) -> io::Result<Registry> {
    let bytes = match fastctx_platform::durable_file::read_bounded(&path(world), RECORD_BYTES as u32) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(Registry { version: VERSION, logins: Vec::new() });
        }
        Err(error) => return Err(error),
    };
    let registry: Registry = serde_json::from_slice(&bytes).map_err(|error| invalid(error.to_string()))?;
    if registry.version == 0 || registry.version > VERSION {
        return Err(invalid(format!("version {} is not one this fastctx reads", registry.version)));
    }
    let mut ids = std::collections::BTreeSet::new();
    let mut dirs = std::collections::BTreeSet::new();
    for login in &registry.logins {
        if !ids.insert(&login.id) || !dirs.insert((&login.agent, &login.dir)) {
            return Err(invalid(format!("{} or {} is given twice", login.id, login.dir.display())));
        }
    }
    Ok(registry)
}

/// The id of `agent`'s login in `dir` on this machine, given and kept now
/// where it had none.
pub(crate) fn of(node: &Node, agent: &AgentName, dir: &Path) -> io::Result<LoginId> {
    let _giving = node.logins.giving_ids.lock().expect("never poisoned");
    given(node.world.path(), agent, dir, RECORD_BYTES)
}

fn given(world: &Path, agent: &AgentName, dir: &Path, bytes_at_most: usize) -> io::Result<LoginId> {
    let real = fastctx_platform::disk::canonicalize(dir)
        .map_err(|error| io::Error::new(error.kind(), format!("resolving {}: {error}", dir.display())))?;
    let mut registry = read(world)?;
    if let Some(known) = registry.logins.iter().find(|known| known.agent == *agent && known.dir == real) {
        return Ok(known.id.clone());
    }
    // Moved: a finished move took the directory kept for one id away and
    // left a link to this one where it was (`super::moves`). A link put
    // there by other hands is another login. Only where one alone is, and
    // the record says where it is from here on.
    let moved_from = super::moves::finished_sources(world);
    let moved: Vec<usize> = registry
        .logins
        .iter()
        .enumerate()
        .filter(|(_, known)| {
            known.agent == *agent
                && moved_from.contains(&known.dir)
                && fastctx_platform::disk::canonicalize(&known.dir).is_ok_and(|now| now == real)
        })
        .map(|(index, _)| index)
        .collect();
    if let [index] = moved[..] {
        registry.logins[index].dir = real;
        let id = registry.logins[index].id.clone();
        keep(world, &registry, bytes_at_most)?;
        return Ok(id);
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
        dir: real,
        written: dir.to_path_buf(),
        at: Timestamp::now(),
    });
    keep(world, &registry, bytes_at_most)?;
    Ok(id)
}

fn keep(world: &Path, registry: &Registry, bytes_at_most: usize) -> io::Result<()> {
    let mut bytes = serde_json::to_vec_pretty(registry).map_err(io::Error::other)?;
    bytes.push(b'\n');
    // Never more than is read back.
    if bytes.len() > bytes_at_most {
        return Err(invalid(format!("would exceed {bytes_at_most} bytes")));
    }
    publish(&path(world), &bytes, Permissions::Private)
}

/// `of`, or none where it could not be read or kept, which the node's log
/// tells: the session is born all the same, as one born before ids were kept.
fn or_told(node: &Node, agent: &str, dir: &Path) -> Option<LoginId> {
    let agent = AgentName::try_from(agent).expect("an agent's name");
    match of(node, &agent, dir) {
        Ok(id) => Some(id),
        Err(error) => {
            node.log.line(format_args!("giving the {agent} login {} its id: {error}", dir.display()));
            None
        }
    }
}

/// The login a Codex session the World starts here is born in: `home`, or
/// the default one this node's Codex runs, whose sign-in an assisting host
/// borrows as well.
pub(crate) fn codex_born(node: &Node, home: Option<&Path>) -> Option<LoginId> {
    let dir = home.map_or_else(|| node.codex.default().config_home().to_path_buf(), Path::to_path_buf);
    or_told(node, "codex", &dir)
}

/// The login a Claude Code session is started in with `env`: the directory
/// it is given there, else the user's default one, as Claude Code finds it.
pub(crate) fn claude_started(node: &Node, env: &[(OsString, OsString)]) -> Option<LoginId> {
    let user_home = match fastctx_platform::user_home() {
        Ok(home) => home,
        Err(error) => {
            node.log.line(format_args!("finding the home directory for a Claude Code login's id: {error}"));
            return None;
        }
    };
    or_told(node, "claude", &crate::homes::claude_default(env, &user_home))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(name: &str) -> AgentName {
        AgentName::try_from(name).expect("an agent's name")
    }

    fn login(world: &Path, name: &str) -> PathBuf {
        let home = world.join(name);
        fastctx_platform::disk::create_dir_all(&home).expect("a login");
        home
    }

    #[test]
    fn a_login_keeps_the_id_it_was_given() {
        let world = tempfile::tempdir().expect("a directory");
        let home = login(world.path(), "claude-home");
        let first = given(world.path(), &agent("claude"), &home, RECORD_BYTES).expect("an id");
        assert!(fastctx_platform::disk::is_file(path(world.path())), "kept before it is answered");
        assert_eq!(given(world.path(), &agent("claude"), &home, RECORD_BYTES).expect("an id"), first);
        // Another spelling of the same directory is the same login.
        let mut spelled = home.clone().into_os_string();
        spelled.push("/");
        assert_eq!(given(world.path(), &agent("claude"), Path::new(&spelled), RECORD_BYTES).expect("an id"), first);
        let around = home.join("..").join("claude-home");
        assert_eq!(given(world.path(), &agent("claude"), &around, RECORD_BYTES).expect("an id"), first);
        // Another agent's login in that directory, or another directory, is another.
        let codex = given(world.path(), &agent("codex"), &home, RECORD_BYTES).expect("an id");
        assert_ne!(codex, first);
        let other = login(world.path(), "other-home");
        let second = given(world.path(), &agent("claude"), &other, RECORD_BYTES).expect("an id");
        assert_ne!(second, first);
        assert_ne!(second, codex);
        assert_eq!(read(world.path()).expect("the record").logins.len(), 3);
        // A directory that is not there has none, and nothing is kept for it.
        assert!(given(world.path(), &agent("claude"), &world.path().join("gone"), RECORD_BYTES).is_err());
        assert_eq!(read(world.path()).expect("the record").logins.len(), 3);
    }

    #[cfg(unix)]
    #[test]
    fn a_link_names_the_login_it_leads_to_now() {
        let world = tempfile::tempdir().expect("a directory");
        let (a, b) = (login(world.path(), "a"), login(world.path(), "b"));
        let link = world.path().join("link");
        std::os::unix::fs::symlink(&a, &link).expect("a link");
        let of_a = given(world.path(), &agent("claude"), &link, RECORD_BYTES).expect("an id");
        assert_eq!(given(world.path(), &agent("claude"), &a, RECORD_BYTES).expect("an id"), of_a);
        fastctx_platform::disk::remove_file(&link).expect("unlinked");
        std::os::unix::fs::symlink(&b, &link).expect("a link");
        let of_b = given(world.path(), &agent("claude"), &link, RECORD_BYTES).expect("an id");
        assert_ne!(of_b, of_a);
        assert_eq!(given(world.path(), &agent("claude"), &b, RECORD_BYTES).expect("an id"), of_b);
    }

    #[cfg(unix)]
    #[test]
    fn a_moved_login_keeps_its_id() {
        let world = tempfile::tempdir().expect("a directory");
        let home = login(world.path(), "claude-home");
        let first = given(world.path(), &agent("claude"), &home, RECORD_BYTES).expect("an id");
        // Moved as `fastctx login move` moves it: copied, the original set
        // aside, a link left where it was.
        let disk = login(world.path(), "disk");
        let went = disk.join("claude-home");
        let real = fastctx_platform::disk::canonicalize(&home).expect("resolved");
        fastctx_platform::disk::rename(&home, world.path().join("claude-home.moved")).expect("set aside");
        fastctx_platform::disk::create_dir_all(&went).expect("copied");
        std::os::unix::fs::symlink(&went, &home).expect("a link");
        // Not yet known to have been moved: another login.
        let unproven = tempfile::tempdir().expect("a directory");
        fastctx_platform::disk::copy(path(world.path()), path(unproven.path())).expect("the record");
        assert_ne!(given(unproven.path(), &agent("claude"), &home, RECORD_BYTES).expect("an id"), first);
        super::super::moves::finished_for_test(world.path(), &real, &went);
        assert_eq!(given(world.path(), &agent("claude"), &home, RECORD_BYTES).expect("an id"), first);
        assert_eq!(given(world.path(), &agent("claude"), &went, RECORD_BYTES).expect("an id"), first);
        let record = read(world.path()).expect("the record");
        assert_eq!(record.logins.len(), 1);
        assert_eq!(record.logins[0].dir, fastctx_platform::disk::canonicalize(&went).expect("resolved"));
        // Moved again: a link left where it went.
        let further = disk.join("again");
        let then = fastctx_platform::disk::canonicalize(&went).expect("resolved");
        fastctx_platform::disk::rename(&went, disk.join("claude-home.moved")).expect("set aside");
        fastctx_platform::disk::create_dir_all(&further).expect("copied");
        std::os::unix::fs::symlink(&further, &went).expect("a link");
        super::super::moves::finished_for_test(world.path(), &then, &further);
        assert_eq!(given(world.path(), &agent("claude"), &home, RECORD_BYTES).expect("an id"), first);
        // Its disk not there: no id, and nothing given anew.
        fastctx_platform::disk::rename(&further, disk.join("away")).expect("away");
        assert!(given(world.path(), &agent("claude"), &home, RECORD_BYTES).is_err());
        assert_eq!(read(world.path()).expect("the record").logins.len(), 1);
    }

    #[test]
    fn a_record_that_cannot_be_read_is_never_written_over() {
        let world = tempfile::tempdir().expect("a directory");
        let home = login(world.path(), "codex-home");
        let entry = |dir: &str| {
            format!(
                r#"{{"id": "l-0000000000000001", "agent": "codex", "dir": "{dir}", "written": "{dir}", "at": "2026-11-20T00:00:00Z"}}"#
            )
        };
        let twice = format!(r#"{{"version": 1, "logins": [{}, {}]}}"#, entry("/a"), entry("/b"));
        for written in [
            &b"{ not json"[..],
            &b"{}"[..],
            &br#"{"version": 2, "logins": []}"#[..],
            &br#"{"version": 0, "logins": []}"#[..],
            twice.as_bytes(),
        ] {
            fastctx_platform::disk::write(path(world.path()), written).expect("a record");
            assert!(given(world.path(), &agent("codex"), &home, RECORD_BYTES).is_err());
            assert_eq!(fastctx_platform::disk::read(path(world.path())).expect("the record"), written);
        }
    }

    #[test]
    fn nothing_is_kept_that_could_not_be_read_back() {
        let world = tempfile::tempdir().expect("a directory");
        let home = login(world.path(), "codex-home");
        assert!(given(world.path(), &agent("codex"), &home, 64).is_err());
        assert!(!fastctx_platform::disk::is_file(path(world.path())));
    }
}
