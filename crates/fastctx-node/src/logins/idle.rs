//! Compacting conversations idle a while (Issue #57), as each account set it
//! for each login here, kept in `<world>/idle-compaction.json`. A login is
//! its directory, as with the models changed in one (`kept`); what one
//! account set is apart from every other's, and none set is off.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};

use fastctx_platform::{Permissions, publish};
use fastctx_types::access::{Acting, HubAccount};
use fastctx_types::ipc::Failure;
use fastctx_types::logins::MachineLogins;
use fastctx_types::logins::idle::IdleCompaction;
use fastctx_types::AgentName;
use serde::{Deserialize, Serialize};

use crate::Node;

const VERSION: u32 = 1;
const RECORD_BYTES: usize = 1024 * 1024;
/// The most settings kept, a guard alone: each is an account's for a login.
const MOST: usize = 4096;

#[derive(Serialize, Deserialize)]
struct Record {
    version: u32,
    settings: Vec<Kept>,
}

#[derive(Clone, Serialize, Deserialize)]
struct Kept {
    of: HubAccount,
    agent: AgentName,
    dir: PathBuf,
    setting: IdleCompaction,
}

/// What each world's record holds, read once and kept as written: the
/// sweep asks it every pass (`crate::idle_compaction`).
static KEPT: LazyLock<Mutex<HashMap<PathBuf, Arc<Vec<Kept>>>>> = LazyLock::new(Default::default);

fn path(world: &Path) -> PathBuf {
    world.join("idle-compaction.json")
}

fn invalid(why: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("idle-compaction.json: {why}"))
}

fn read(world: &Path) -> io::Result<Vec<Kept>> {
    let bytes = match fastctx_platform::durable_file::read_bounded(&path(world), RECORD_BYTES as u32) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let record: Record = serde_json::from_slice(&bytes).map_err(|error| invalid(error.to_string()))?;
    if record.version == 0 || record.version > VERSION {
        return Err(invalid(format!("version {} is not one this fastctx reads", record.version)));
    }
    Ok(record.settings)
}

fn kept(node: &Node) -> io::Result<Arc<Vec<Kept>>> {
    let world = node.world.path();
    let mut all = KEPT.lock().expect("never poisoned");
    if let Some(settings) = all.get(world) {
        return Ok(settings.clone());
    }
    let settings = Arc::new(read(world)?);
    all.insert(world.to_path_buf(), settings.clone());
    Ok(settings)
}

/// The accounts a session working for `acting` works for, in the order
/// their settings are looked at: this computer's own for a session the user
/// opened here.
pub(crate) fn accounts_of(node: &Node, acting: &Acting) -> Vec<HubAccount> {
    match acting {
        Acting::Here => own(node),
        Acting::Accounts { accounts } => accounts.clone(),
    }
}

/// The accounts logged in on this computer, on each of its hubs.
pub(crate) fn own(node: &Node) -> Vec<HubAccount> {
    let mut own = Vec::new();
    for link in node.links() {
        let hub = link.membership().id.clone();
        if let Some(me) = link.view().me() {
            own.extend(me.logins.iter().map(|account| HubAccount { hub: hub.clone(), account: account.clone() }));
        }
    }
    own
}

/// The setting of `agent`'s login `dir` for the first of `accounts` that set
/// one; none where none did.
pub(crate) fn of(node: &Node, accounts: &[HubAccount], agent: &str, dir: &Path) -> Option<IdleCompaction> {
    let settings = match kept(node) {
        Ok(settings) => settings,
        Err(error) => {
            node.log.line(format_args!("reading the idle compaction settings: {error}"));
            return None;
        }
    };
    accounts.iter().find_map(|account| {
        settings
            .iter()
            .find(|kept| kept.of == *account && kept.agent.as_str() == agent && crate::homes::same_dir(&kept.dir, dir))
            .map(|kept| kept.setting)
    })
}

/// Whether any account turned it on for any login here: the sweep looks no
/// further while none did.
pub(crate) fn any_on(node: &Node) -> bool {
    kept(node).is_ok_and(|settings| settings.iter().any(|kept| kept.setting.on))
}

/// Shows each listed login of `logins` with what the first of `accounts`
/// that set it set.
pub(crate) fn fill(node: &Node, accounts: &[HubAccount], logins: &mut MachineLogins) {
    for agent in super::AGENTS {
        let Some(side) = logins.of_mut(agent) else { continue };
        for login in &mut side.listed {
            login.idle_compaction = of(node, accounts, agent, &login.dir);
        }
    }
}

/// Keeps `setting` for `agent`'s login `dir` here for each of `accounts`.
pub(crate) fn set(
    node: &Node,
    accounts: &[HubAccount],
    agent: &AgentName,
    dir: &Path,
    setting: IdleCompaction,
) -> Result<(), Failure> {
    if let Some(why) = setting.problem() {
        return Err(Failure::not_run(why));
    }
    if accounts.is_empty() {
        return Err(Failure::not_run("no account to keep it for"));
    }
    super::known(node, agent, dir)?;
    let world = node.world.path();
    let _keeping = node.logins.keeping.lock().expect("never poisoned");
    let mut settings = read(world).map_err(|error| Failure::new(error.to_string()))?;
    for account in accounts {
        let at = settings
            .iter()
            .position(|kept| kept.of == *account && kept.agent == *agent && crate::homes::same_dir(&kept.dir, dir));
        match at {
            Some(at) => settings[at].setting = setting,
            None => settings.push(Kept { of: account.clone(), agent: agent.clone(), dir: dir.to_path_buf(), setting }),
        }
    }
    if settings.len() > MOST {
        return Err(Failure::not_run(format!("more than {MOST} settings would be kept")));
    }
    let mut bytes = serde_json::to_vec_pretty(&Record { version: VERSION, settings: settings.clone() })
        .map_err(|error| Failure::new(error.to_string()))?;
    bytes.push(b'\n');
    if bytes.len() > RECORD_BYTES {
        return Err(Failure::not_run(format!("the settings would exceed {RECORD_BYTES} bytes")));
    }
    publish(&path(world), &bytes, Permissions::Private).map_err(|error| Failure::new(error.to_string()))?;
    KEPT.lock().expect("never poisoned").insert(world.to_path_buf(), Arc::new(settings));
    Ok(())
}
