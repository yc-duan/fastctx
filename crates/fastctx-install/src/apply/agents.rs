//! Connecting the agents of `crate::agents`, and taking them out, on disk:
//! under the apply lock, the files the agent's connecting touches are read,
//! the change worked out from them, and written with the agent's own record
//! in one durable transaction. Each connected home keeps its record in a
//! file of its own, `applied-agent-<name>-<digest>.json` beside
//! `applied.json`, which releases before these never read.

use std::path::{Path, PathBuf};

use fastctx_platform::FileLock;
use fastctx_types::AgentName;
use fastctx_types::logins::{ChangePreview, Kept};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::progress::{self, Progress};
use super::{Error, Places, Report, change, homes, io, placing, recovery, tell, text};
use crate::agents::{self, Agent, Files, Planned, Receipt, Server};
use crate::transaction::{CreatedDirectory, FileChange, Snapshot, Transaction};

const VERSION: u32 = 1;

/// What connecting one home of one agent wrote, and the directories its
/// transaction made, to take out again.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u32,
    agent: String,
    home: PathBuf,
    receipt: Receipt,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    directories: Vec<CreatedDirectory>,
}

/// The agent's home as connecting reads it, and the record kept for it.
struct Read {
    home: PathBuf,
    record_file: Snapshot,
    record: Option<Record>,
    files: Files,
    snapshots: Vec<(&'static str, Snapshot)>,
}

fn record_path(places: &Places, agent: &Agent, home: &Path) -> PathBuf {
    let spelled = home.to_string_lossy();
    let digest: String = Sha256::digest(spelled.as_bytes())[..8].iter().map(|byte| format!("{byte:02x}")).collect();
    places.root.join(format!("applied-agent-{}-{digest}.json", agent.name))
}

fn read(places: &Places, agent: &Agent, home: &Path) -> Result<Read, Error> {
    let home = homes::resolve(home)?;
    let record_file = Snapshot::read(record_path(places, agent, &home))?;
    let record = match record_file.bytes() {
        None => None,
        Some(bytes) => {
            let record: Record = serde_json::from_slice(bytes)
                .map_err(|error| Error::Io(format!("{}: {error}", record_file.path().display())))?;
            if record.version != VERSION || record.agent != agent.name {
                return Err(Error::Io(format!(
                    "{}: not a record of this version for {}",
                    record_file.path().display(),
                    agent.name
                )));
            }
            Some(record)
        }
    };
    let mut files = Files::new();
    let mut snapshots = Vec::new();
    for name in agent.files() {
        let snapshot = Snapshot::read(home.join(name))?;
        files.insert(name, (text(snapshot.path()), snapshot.bytes().map(<[u8]>::to_vec)));
        snapshots.push((name, snapshot));
    }
    Ok(Read { home, record_file, record, files, snapshots })
}

/// How the agent starts Fastctx's server: the World's own copy of fastctx,
/// serving, told the World and the agent's home.
fn server_of(places: &Places, home: &Path) -> (String, Vec<String>, Vec<(String, String)>) {
    let command = text(&places.installed());
    let env = vec![("FASTCTX_HOME".to_owned(), text(&places.root)), ("FASTCTX_AGENT_HOME".to_owned(), text(home))];
    (command, vec!["serve".to_owned()], env)
}

fn plan(places: &Places, agent: &Agent, read: &Read, into: bool, take_back: bool) -> Result<Option<Planned>, Error> {
    if into {
        let (command, args, env) = server_of(places, &read.home);
        let server = Server { command: &command, args: &args, env: &env };
        let previous = read.record.as_ref().map(|record| &record.receipt);
        let body = crate::briefing::guidance(false);
        return Ok(Some(agents::connect(agent, &read.files, previous, &server, &body, take_back)?));
    }
    match &read.record {
        Some(record) => Ok(Some(agents::disconnect(agent, &read.files, &record.receipt)?)),
        None => Ok(None),
    }
}

/// What the change is worked out from, so that the change made is the one
/// shown (`crate::connect::fingerprint`).
fn fingerprint_of(read: &Read) -> String {
    let mut inputs = vec![(read.record_file.path(), read.record_file.bytes())];
    inputs.extend(read.snapshots.iter().map(|(_, snapshot)| (snapshot.path(), snapshot.bytes())));
    crate::connect::fingerprint(&inputs)
}

/// What connecting `agent` in `home` (`into`), or taking it out, would
/// change, with nothing written.
pub fn agent_preview(
    places: &Places,
    agent: &Agent,
    home: &Path,
    into: bool,
    take_back: bool,
) -> Result<ChangePreview, Error> {
    let read = read(places, agent, home)?;
    let planned = plan(places, agent, &read, into, take_back)?;
    let name = AgentName::try_from(agent.name).map_err(|error| Error::Io(format!("{}: {error}", agent.name)))?;
    Ok(ChangePreview {
        agent: name,
        dir: read.home.clone(),
        into,
        files: Vec::new(),
        commands: Vec::new(),
        output_limit: None,
        kept: planned
            .as_ref()
            .map(|planned| {
                planned
                    .notices
                    .iter()
                    .map(|notice| Kept { location: notice.location.clone(), reason: notice.reason.clone() })
                    .collect()
            })
            .unwrap_or_default(),
        cards: planned.as_ref().map(|planned| planned.steps.cards()).unwrap_or_default(),
        fingerprint: Some(fingerprint_of(&read)),
    })
}

/// The fingerprint of what connecting `agent` in `home` reads now.
pub fn agent_fingerprint(places: &Places, agent: &Agent, home: &Path) -> Result<String, Error> {
    Ok(fingerprint_of(&read(places, agent, home)?))
}

/// Whether Fastctx is connected to `agent` in `home`.
pub fn agent_connected(places: &Places, agent: &Agent, home: &Path) -> Result<bool, Error> {
    let home = homes::resolve(home)?;
    let file = Snapshot::read(record_path(places, agent, &home))?;
    Ok(match file.bytes() {
        None => false,
        Some(bytes) => serde_json::from_slice::<Record>(bytes).is_ok_and(|record| record.receipt.holds()),
    })
}

/// Every home of these agents Fastctx is connected to: the agent's name and
/// the home, as their records say.
pub fn agent_integrations(places: &Places) -> Result<Vec<(String, PathBuf)>, Error> {
    let entries = match std::fs::read_dir(&places.root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(Error::Io(format!("reading {}: {error}", places.root.display()))),
    };
    let mut found = Vec::new();
    for entry in entries {
        let entry = entry.map_err(io("listing fastctx's directory"))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !(name.starts_with("applied-agent-") && name.ends_with(".json")) {
            continue;
        }
        let Ok(bytes) = std::fs::read(entry.path()) else { continue };
        if let Ok(record) = serde_json::from_slice::<Record>(&bytes)
            && agents::find(&record.agent).is_some()
            && record.receipt.holds()
        {
            found.push((record.agent, record.home));
        }
    }
    found.sort();
    Ok(found)
}

/// Connects `agent` in `home`, made only while the files are as they were
/// when the change was shown (`expected`); with `take_back`, what the user
/// changed of Fastctx's is written as this version writes it.
pub fn agent_apply_expecting(
    places: &Places,
    agent: &Agent,
    home: &Path,
    take_back: bool,
    progress: Option<Progress>,
    expected: Option<&str>,
) -> Result<Report, Error> {
    if places.root.join("uninstalled.json").try_exists().map_err(io("checking uninstall state"))? {
        return Err(Error::Io("this World was uninstalled; reinstall it before connecting agents".into()));
    }
    tell(&progress, progress::Step::Program, progress::StepState::Running);
    crate::placement::ensure(&places.root, &places.fastctx).map_err(placing)?;
    tell(&progress, progress::Step::Program, progress::StepState::Done);
    made(places, agent, home, true, take_back, progress, expected)
}

/// Takes `agent` out of `home`, made only while the files are as they were
/// when the change was shown (`expected`).
pub fn agent_unapply_expecting(
    places: &Places,
    agent: &Agent,
    home: &Path,
    progress: Option<Progress>,
    expected: Option<&str>,
) -> Result<Report, Error> {
    made(places, agent, home, false, false, progress, expected)
}

fn made(
    places: &Places,
    agent: &Agent,
    home: &Path,
    into: bool,
    take_back: bool,
    progress: Option<Progress>,
    expected: Option<&str>,
) -> Result<Report, Error> {
    crate::disk::ensure_private_dir(&places.root).map_err(io("making fastctx's directory"))?;
    let _lock = FileLock::acquire(&places.lock()).map_err(io("taking the apply lock"))?;
    if places.root.join("uninstalled.json").try_exists().map_err(io("checking uninstall state"))? {
        return Err(Error::Io("this World is being uninstalled".into()));
    }
    let mut report = recovery::before(places)?;
    let read = read(places, agent, home)?;
    if let Some(expected) = expected
        && fingerprint_of(&read) != expected
    {
        return Err(Error::Moved);
    }
    let Some(planned) = plan(places, agent, &read, into, take_back)? else {
        return Ok(report);
    };
    let operation = recovery::id()?;
    let mut changes = Vec::new();
    for (name, snapshot) in read.snapshots {
        let after = planned.files.get(name).cloned().flatten();
        if snapshot.bytes().is_none() && after.is_none() {
            continue;
        }
        change(&mut changes, snapshot, after);
    }
    report.notices.extend(planned.notices.iter().cloned());
    changes.push(recovery::stamp(places, &operation)?);
    tell(&progress, progress::Step::Files { changed: Vec::new() }, progress::StepState::Running);
    let committed = if into {
        let mut record = Record {
            version: VERSION,
            agent: agent.name.to_owned(),
            home: read.home.clone(),
            receipt: planned.receipt,
            directories: read.record.map(|record| record.directories).unwrap_or_default(),
        };
        Transaction::new(changes).commit_with_record_durable(places.journal(), read.record_file, |created| {
            record.directories.extend(created.directories.iter().cloned());
            serde_json::to_vec_pretty(&record).map_err(std::io::Error::other)
        })
    } else {
        let directories = read.record.map(|record| record.directories).unwrap_or_default();
        changes.push(FileChange::delete(read.record_file));
        Transaction::new(changes).cleanup_after_commit(&directories).commit_durable(places.journal())
    };
    match committed {
        Ok(receipt) => {
            tell(&progress, progress::Step::Files { changed: receipt.changed.clone() }, progress::StepState::Done);
            report.changed.extend(receipt.changed);
            report.preserved.extend(receipt.preserved);
            report.leftovers.extend(receipt.cleanup.iter().map(ToString::to_string));
            Ok(report)
        }
        Err(failure) => Err(Error::Files(failure.to_string())),
    }
}
