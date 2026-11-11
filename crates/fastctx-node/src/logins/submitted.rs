//! User-requested login work, recorded before scans, host probes or file changes.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use fastctx_types::AgentName;
use fastctx_types::ipc::Failure;
use fastctx_types::logins::{FolderChange, LoginChange, MachineLogins};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::Node;

/// Installation can leave files changed, and identifying or reading a login may
/// already have asked its host. An unfinished try is reported, never replayed.
const FAMILY: crate::submitted::Family = crate::submitted::Family {
    name: "call/logins",
    at_once: 8,
    // Host probes and installation are expected within the previous change-call deadline.
    within: Duration::from_secs(5 * 60),
    // Reading limits has the shortest call deadline, 150 seconds.
    stays: Duration::from_secs(120),
    again: false,
};

/// A usual-places scan is machine-wide; directory searches have separate units.
const SCAN_UNIT: &str = "login-scan";

#[derive(Serialize, Deserialize)]
enum Asked {
    Add {
        agent: AgentName,
        dir: PathBuf,
    },
    AddHome {
        agent: AgentName,
        dir: PathBuf,
    },
    Change {
        agent: AgentName,
        dir: PathBuf,
        into: bool,
        raise_output_limit: bool,
        take_back: bool,
        expected: Option<String>,
        workbench: Option<bool>,
    },
    Choose {
        dir: PathBuf,
    },
    Scan,
    Limits {
        agent: AgentName,
        dir: PathBuf,
        from_record: bool,
    },
    Resets {
        agent: AgentName,
        dir: PathBuf,
        on: bool,
    },
}

#[derive(Serialize, Deserialize)]
enum Answer {
    Logins(Box<MachineLogins>),
    Changed(LoginChange),
    Set,
}

pub(crate) fn register(node: &Arc<Node>) {
    crate::submitted::register(node, FAMILY, |node: Arc<Node>, asked: Asked| async move { work(&node, asked).await });
}

/// Uses the node's existing path spelling rules. A full digest fits the job's
/// unit budget even for long paths; the original spelling remains in Asked.
fn directory(dir: &Path) -> String {
    format!("login-dir/{}", path_key(dir))
}

fn folder_unit(dir: &Path) -> String {
    // Stopping a folder must not withdraw installation or quota work in it.
    format!("login-folder/{}", path_key(dir))
}

fn path_key(dir: &Path) -> String {
    let key = fastctx_hosts::inventory::walk::key(dir);
    Sha256::digest(key.as_bytes()).iter().map(|byte| format!("{byte:02x}")).collect()
}

pub(super) async fn withdraw_folder(node: &Node, dir: &Path, why: &str) -> Result<(), Failure> {
    crate::submitted::withdraw(node, &FAMILY, &folder_unit(dir), why).await.map(|_| ())
}

pub(super) async fn withdraw_scan(node: &Node) -> Result<(), Failure> {
    crate::submitted::withdraw(node, &FAMILY, SCAN_UNIT, "the scan was stopped before it began").await.map(|_| ())
}

fn absolute(dir: &Path) -> Result<(), Failure> {
    if dir.is_absolute() { Ok(()) } else { Err(Failure::not_run(format!("{} is not an absolute path", dir.display()))) }
}

fn agent_known(agent: &AgentName) -> Result<(), Failure> {
    if fastctx_install::agents::find(agent.as_str()).is_none() {
        super::host(agent.as_str())?;
    }
    Ok(())
}

async fn submit(node: &Arc<Node>, unit: &str, asked: &Asked) -> Result<Answer, Failure> {
    crate::submitted::submit(node, &FAMILY, unit, asked).await
}

fn logins(answer: Answer) -> Result<MachineLogins, Failure> {
    match answer {
        Answer::Logins(logins) => Ok(*logins),
        Answer::Changed(_) | Answer::Set => Err(Failure::new("the login operation returned a different answer")),
    }
}

pub(crate) async fn add(node: &Arc<Node>, agent: AgentName, dir: PathBuf) -> Result<MachineLogins, Failure> {
    super::host(agent.as_str())?;
    absolute(&dir)?;
    logins(submit(node, &directory(&dir), &Asked::Add { agent, dir }).await?)
}

pub(crate) async fn add_home(node: &Arc<Node>, agent: AgentName, dir: PathBuf) -> Result<(), Failure> {
    super::host(agent.as_str())?;
    absolute(&dir)?;
    match submit(node, &directory(&dir), &Asked::AddHome { agent, dir }).await? {
        Answer::Set => Ok(()),
        Answer::Logins(_) | Answer::Changed(_) => Err(Failure::new("the home operation returned a different answer")),
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn change(
    node: &Arc<Node>,
    agent: AgentName,
    dir: PathBuf,
    into: bool,
    raise_output_limit: bool,
    take_back: bool,
    expected: Option<String>,
    workbench: Option<bool>,
) -> Result<LoginChange, Failure> {
    agent_known(&agent)?;
    let unit = directory(&dir);
    let asked = Asked::Change { agent, dir, into, raise_output_limit, take_back, expected, workbench };
    match submit(node, &unit, &asked).await? {
        Answer::Changed(change) => Ok(change),
        Answer::Logins(_) | Answer::Set => Err(Failure::new("the login change returned a different answer")),
    }
}

pub(super) async fn folder(node: &Arc<Node>, dir: PathBuf, change: FolderChange) -> Result<MachineLogins, Failure> {
    if !matches!(change, FolderChange::Choose) {
        return super::folders::change(node, dir, change).await;
    }
    absolute(&dir)?;
    logins(submit(node, &folder_unit(&dir), &Asked::Choose { dir }).await?)
}

pub(super) async fn scan(node: &Arc<Node>, stop: bool) -> Result<MachineLogins, Failure> {
    if stop {
        return super::scan::scan(node, true).await;
    }
    if node.logins.scanning.load(std::sync::atomic::Ordering::Acquire) {
        return super::here(node).await;
    }
    logins(submit(node, SCAN_UNIT, &Asked::Scan).await?)
}

pub(super) async fn read_limits(
    node: &Arc<Node>,
    agent: AgentName,
    dir: PathBuf,
    from_record: bool,
) -> Result<MachineLogins, Failure> {
    if !super::workbench::is_open(node, &fastctx_types::access::Acting::Here, agent.as_str(), Some(&dir)) {
        return Err(Failure::refused(super::workbench::CLOSED));
    }
    if agent.as_str() != "codex" {
        return Err(Failure::not_run(
            "Claude Code reads its limits only in a session at work; they are shown as its sessions last read them",
        ));
    }
    logins(submit(node, &directory(&dir), &Asked::Limits { agent, dir, from_record }).await?)
}

pub(super) async fn set_resets_in_place(
    node: &Arc<Node>,
    agent: AgentName,
    dir: PathBuf,
    on: bool,
) -> Result<MachineLogins, Failure> {
    if !super::AGENTS.contains(&agent.as_str()) {
        return Err(Failure::not_run("only Codex and Claude Code logins keep their usage"));
    }
    logins(submit(node, &directory(&dir), &Asked::Resets { agent, dir, on }).await?)
}

/// Calls the original implementations directly: choosing a folder may take up
/// a login, so sending that nested call through this family would wait on itself.
async fn work(node: &Arc<Node>, asked: Asked) -> Result<Answer, Failure> {
    match asked {
        Asked::Add { agent, dir } => {
            super::take_up(node, agent, dir).await.map(|logins| Answer::Logins(Box::new(logins)))
        }
        Asked::AddHome { agent, dir } => super::take_up(node, agent, dir).await.map(|_| Answer::Set),
        Asked::Change { agent, dir, into, raise_output_limit, take_back, expected, workbench } => {
            super::change::change(node, agent, dir, into, raise_output_limit, take_back, expected, workbench)
                .await
                .map(Answer::Changed)
        }
        Asked::Choose { dir } => {
            super::folders::change(node, dir, FolderChange::Choose).await.map(|logins| Answer::Logins(Box::new(logins)))
        }
        Asked::Scan => super::scan::scan(node, false).await.map(|logins| Answer::Logins(Box::new(logins))),
        Asked::Limits { agent, dir, from_record } => {
            let dir = if from_record {
                let record = super::here(node).await?;
                let side =
                    record.of(agent.as_str()).ok_or_else(|| Failure::not_run(format!("{agent} keeps no logins")))?;
                side.listed
                    .iter()
                    .chain(&side.candidates)
                    .find(|login| crate::homes::same_dir(&login.dir, &dir))
                    .map(|login| login.dir.clone())
                    .ok_or_else(|| {
                        Failure::not_run(format!("no {agent} login {} here is the account's to see", dir.display()))
                    })?
            } else {
                dir
            };
            super::limits::read(node, &agent, &dir).await?;
            super::changed_here(node).await.map(|logins| Answer::Logins(Box::new(logins)))
        }
        Asked::Resets { agent, dir, on } => {
            super::set_resets_in_place(node, &agent, &dir, on)?;
            crate::usage::after_reading(node, agent.as_str(), &dir).await;
            super::changed_here(node).await.map(|logins| Answer::Logins(Box::new(logins)))
        }
    }
}
