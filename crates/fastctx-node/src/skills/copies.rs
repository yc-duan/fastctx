//! Account-owned, read-only skill generations. A complete folder is published
//! before its path is returned. Old generations survive while their users have
//! running jobs; edits on disk never become account-record edits.

use crate::{Node, admission::Asker};
use fastctx_types::{
    Address,
    access::{Acting, HubAccount},
    ipc::Failure,
    placement::Location,
    skill_files::{Limits, Reply, Request},
    skills::Skill,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

mod follow;
pub(crate) use follow::{changed, refresh, start};

#[derive(Default)]
pub(crate) struct Copies {
    accounts: Mutex<HashMap<HubAccount, Arc<tokio::sync::Mutex<()>>>>,
    held: Mutex<HashMap<(HubAccount, String), Arc<follow::Held>>>,
}
#[derive(Default, Serialize, Deserialize)]
struct Kept {
    generations: BTreeMap<String, Generation>,
}
#[derive(Serialize, Deserialize)]
struct Generation {
    name: String,
    version: u64,
    size: u64,
    users: Vec<Address>,
}
#[derive(Serialize, Deserialize)]
struct Identity {
    account: HubAccount,
    opened: jiff::Timestamp,
}
fn failure(error: impl std::fmt::Display) -> Failure {
    Failure::new(error.to_string())
}
fn root(node: &Node, account: &HubAccount, opened: jiff::Timestamp) -> Result<PathBuf, Failure> {
    let identity = Identity { account: account.clone(), opened };
    Ok(node
        .world
        .path()
        .join("skill-copies")
        .join(format!("{:x}", Sha256::digest(serde_json::to_vec(&identity).map_err(failure)?))))
}
fn load(root: &Path) -> Result<Kept, Failure> {
    match std::fs::read(root.join("generations.json")) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(failure),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Kept::default()),
        Err(error) => Err(failure(error)),
    }
}
fn keep(root: &Path, kept: &Kept) -> Result<(), Failure> {
    fastctx_platform::publish(
        &root.join("generations.json"),
        &serde_json::to_vec(kept).map_err(failure)?,
        fastctx_platform::Permissions::Private,
    )
    .map_err(failure)
}
use crate::placement::files::{erase, unpack};

pub(crate) async fn prepare(
    node: &Arc<Node>,
    asker: &Asker,
    owner: Option<&Address>,
    name: &str,
    expected: Option<u64>,
) -> Result<(Skill, Location), Failure> {
    if !fastctx_types::skills::valid_name(name) {
        return Err(failure("invalid skill name"));
    }
    let scope = HubAccount { hub: asker.hub.clone(), account: asker.account.clone() };
    let acting = Acting::one(scope.hub.clone(), scope.account.clone());
    node.authorize_machine(&acting, &node.machine().into())?;
    let link = node.link_of(&scope.hub).ok_or_else(|| failure("the skill's hub is unavailable"))?;
    let view = crate::authorization::current_view(&link)?;
    if !crate::placement::owned(view.member(&node.machine()), &scope.account) {
        return Err(Failure::refused("skill folders belong on the account's own working computers"));
    }
    let opened = view.account(&scope.account).ok_or_else(|| failure("the skill account is unavailable"))?.opened;
    drop(view);
    let gate = node.skill_copies.accounts.lock().expect("never poisoned").entry(scope.clone()).or_default().clone();
    let one = gate.lock_owned().await;
    let root = root(node, &scope, opened)?;
    follow::register(node, &scope, name);
    let snapshot = match link.skill_files(&scope.account, Request::Read { name: name.into() }).await.map_err(failure)? {
        Reply::Read { snapshot } => snapshot,
        _ => return Err(failure("the hub did not return a skill folder")),
    };
    let node = node.clone();
    let name = name.to_owned();
    let owner = owner.cloned();
    tokio::task::spawn_blocking(move || {
        let _one = one;
        let (name, owner, node) = (name.as_str(), owner.as_ref(), &node);
        let Some(skill) = snapshot.skill else {
            let mut kept = load(&root)?;
            let removed: Vec<_> = kept
                .generations
                .iter()
                .filter(|(_, generation)| generation.name == name)
                .map(|(key, _)| key.clone())
                .collect();
            for key in &removed {
                if crate::placement::jobs::busy(node, &kept.generations[key].users)? {
                    return Err(failure("the skill was deleted; its old folder waits for running jobs to finish"));
                }
            }
            for key in removed {
                erase(&root.join(&key)).map_err(failure)?;
                kept.generations.remove(&key);
            }
            if root.is_dir() {
                keep(&root, &kept)?;
            }
            node.skill_copies.held.lock().expect("never poisoned").remove(&(scope, name.into()));
            return Err(failure(fastctx_types::skills::words::missing(name)));
        };
        let version = snapshot.metadata.ok_or_else(|| failure("the skill has no version"))?.version;
        if expected.is_some_and(|expected| expected != version) {
            return Err(Failure::not_run("the skill changed while its folder was being prepared; read it again"));
        }
        let key = format!("{name}-{version}");
        let limits = Limits::resources(
            fastctx_platform::memory::physical().map_err(failure)?,
            fastctx_platform::space::available(&root).map_err(failure)?,
        );
        let files = skill.folder().map_err(failure)?;
        let size = fastctx_types::skill_files::validate_files(&files).map_err(failure)?;
        if size > limits.skill_bytes || files.len() as u64 > limits.files {
            return Err(failure(format!(
                "the skill exceeds this computer's limits ({} bytes, {} files)",
                limits.skill_bytes, limits.files
            )));
        }
        fastctx_platform::ensure_private_dir(&root).map_err(failure)?;
        fastctx_platform::publish(
            &root.join("account.json"),
            &serde_json::to_vec(&Identity { account: scope.clone(), opened }).map_err(failure)?,
            fastctx_platform::Permissions::Private,
        )
        .map_err(failure)?;
        let mut kept = load(&root)?;
        for entry in std::fs::read_dir(&root).map_err(failure)? {
            let entry = entry.map_err(failure)?;
            if entry.file_type().map_err(failure)?.is_dir()
                && !kept.generations.contains_key(&entry.file_name().to_string_lossy().to_string())
            {
                erase(&entry.path()).map_err(failure)?;
            }
        }
        let mut users = Vec::new();
        for user in kept
            .generations
            .values()
            .filter(|generation| generation.name == name)
            .flat_map(|generation| generation.users.iter())
        {
            if !users.contains(user) {
                users.push(user.clone());
            }
        }
        let mut obsolete = Vec::new();
        for (id, generation) in &kept.generations {
            if *id != key && generation.name == name && !crate::placement::jobs::busy(node, &generation.users)? {
                obsolete.push(id.clone());
            }
        }
        for id in obsolete {
            erase(&root.join(&id)).map_err(failure)?;
            kept.generations.remove(&id);
        }
        let complete = crate::placement::files::complete(&root.join(&key), &files).map_err(failure)?;
        if !kept.generations.contains_key(&key) || !complete {
            let used: u64 =
                kept.generations.iter().filter(|(id, _)| **id != key).map(|(_, generation)| generation.size).sum();
            if used.saturating_add(size) > limits.account_bytes {
                return Err(failure(format!(
                    "skill copies exceed this computer's account allowance of {} bytes",
                    limits.account_bytes
                )));
            }
            let stage = root.join(".arriving");
            erase(&stage).map_err(failure)?;
            let destination = root.join(&key);
            unpack(&stage, &files).map_err(failure)?;
            erase(&destination).map_err(failure)?;
            std::fs::rename(&stage, &destination).map_err(failure)?;
            kept.generations.insert(key.clone(), Generation { name: name.into(), version, size, users });
        }
        if let Some(owner) = owner {
            let generation = kept.generations.get_mut(&key).expect("materialized");
            if !generation.users.contains(owner) {
                generation.users.push(owner.clone());
            }
        }
        let metadata = serde_json::to_vec(&kept).map_err(failure)?.len() as u64;
        let held = kept
            .generations
            .values()
            .try_fold(metadata, |used, generation| used.checked_add(generation.size))
            .ok_or_else(|| failure("the skill-copy size overflows"))?;
        if held > limits.account_bytes {
            return Err(failure(format!(
                "skill folders and their tracking records exceed this computer's account allowance of {} bytes",
                limits.account_bytes
            )));
        }
        keep(&root, &kept)?;
        // Recheck after the download and disk publication; revocation never returns a path.
        node.authorize_machine(&acting, &node.machine().into())?;
        if !crate::placement::owned(crate::authorization::current_view(&link)?.member(&node.machine()), &scope.account)
        {
            return Err(Failure::refused("this computer no longer owns the skill account"));
        }
        follow::register(node, &scope, name);
        Ok((skill, Location { machine: node.machine(), path: crate::world::native(&root.join(key)) }))
    })
    .await
    .map_err(failure)?
}
