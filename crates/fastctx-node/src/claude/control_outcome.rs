//! Evidence for a submitted control belongs to the operation, not the mod's lifetime.

use std::fs::File;
use std::io::{self, BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use fastctx_hosts::claude::{control as files, exchange};
use fastctx_types::ProcessIdentity;
use fastctx_types::ipc::Failure;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Default, Deserialize, Serialize)]
pub(crate) struct Evidence {
    #[serde(default)]
    control: Value,
    compact: bool,
    process: Option<ProcessIdentity>,
    #[serde(default)]
    ended: Option<PathBuf>,
    transcript: Option<Boundary>,
}

#[derive(Clone, Deserialize, Serialize)]
struct Boundary {
    path: PathBuf,
    offset: u64,
    parent: Option<String>,
}

pub(super) fn compacts(control: &Value) -> bool {
    matches!(control["do"].as_str(), Some("compact" | "goal_compact"))
        || (control["do"] == "command" && control["name"] == "compact")
}

impl Evidence {
    pub(crate) fn capture(directory: &Path, control: &Value) -> io::Result<Self> {
        let compact = compacts(control);
        let session = directory.parent().expect("control directory");
        let registration = exchange::registration(session)?;
        let ended = registration
            .as_ref()
            .and_then(|registration| registration.instance.as_ref())
            .map(|instance| session.join("closed").join(instance));
        let process = registration.and_then(|registration| registration.process);
        let transcript = if compact || rename(control).is_some() {
            exchange::transcript(session)?
                .map(|path| -> io::Result<_> {
                    let mut reader = BufReader::new(File::open(&path)?);
                    let mut parent = None;
                    let mut offset = 0;
                    let mut line = String::new();
                    loop {
                        line.clear();
                        let size = reader.read_line(&mut line)?;
                        if size == 0 || !line.ends_with('\n') {
                            break;
                        }
                        let row: Value = serde_json::from_str(&line).map_err(io::Error::other)?;
                        if row["isSidechain"] != true
                            && row.get("agentId").is_none_or(Value::is_null)
                            && matches!(row["type"].as_str(), Some("user" | "assistant" | "attachment" | "system"))
                            && let Some(uuid) = row["uuid"].as_str()
                        {
                            parent = Some(uuid.to_owned());
                        }
                        offset += size as u64;
                    }
                    Ok(Boundary { path, offset, parent })
                })
                .transpose()?
        } else {
            None
        };
        let control = if rename(control).is_some()
            || control["do"] == "model"
            || (control["do"] == "command"
                && matches!(control["name"].as_str(), Some("model" | "reload-plugins" | "exit" | "quit")))
        {
            control.clone()
        } else {
            Value::Null
        };
        Ok(Self { control, compact, process, ended, transcript })
    }

    /// A replacement boundary must consume the context this request saw. A
    /// different session, earlier compaction or later unrelated prefix cannot settle it.
    pub(super) fn completed(&self) -> io::Result<bool> {
        let Some(boundary) = &self.transcript else { return Ok(false) };
        let Some(parent) = &boundary.parent else { return Ok(false) };
        let mut file = File::open(&boundary.path)?;
        if file.metadata()?.len() < boundary.offset {
            return Err(io::Error::other("Claude's transcript was replaced during the control"));
        }
        file.seek(SeekFrom::Start(boundary.offset))?;
        let mut reader = BufReader::new(file);
        let mut line = String::new();
        let mut compacted = None;
        loop {
            line.clear();
            if reader.read_line(&mut line)? == 0 || !line.ends_with('\n') {
                return Ok(false);
            }
            let row: Value = serde_json::from_str(&line).map_err(io::Error::other)?;
            if row["type"] == "system"
                && row["subtype"] == "compact_boundary"
                && row["logicalParentUuid"].as_str() == Some(parent)
                && row.pointer("/compactMetadata/trigger").and_then(Value::as_str) == Some("manual")
                && row["isSidechain"] != true
            {
                compacted = row["uuid"].as_str().map(str::to_owned);
            }
            if row["isCompactSummary"] == true && compacted.as_deref().is_some_and(|uuid| row["parentUuid"] == uuid) {
                return Ok(true);
            }
        }
    }

    pub(super) fn recovered(&self, directory: &Path, key: &str, host: &files::Host) -> io::Result<Option<Value>> {
        if self.compact {
            return Ok(self.completed()?.then(done));
        }
        if let Some(name) = rename(&self.control)
            && self.renamed(name)?
        {
            return Ok(Some(done()));
        }
        let selected = if self.control["do"] == "model" {
            Some((self.control["model"].as_str(), self.control["effort"].as_str()))
        } else if self.control["do"] == "command" && self.control["name"] == "model" {
            self.control["args"].as_str().map(|args| (Some(args.trim()), None))
        } else {
            None
        };
        if let Some((model, effort)) = selected
            && (model.is_some() || effort.is_some())
            && let Some(selection) = files::read_mod::<Value>(&directory.join("selection.json"))?
            && model.is_none_or(|model| selection["model"] == model)
            && effort.is_none_or(|effort| selection["effort"] == effort)
            && (model.is_none() || files::read_mod::<Value>(&directory.join("routing.json"))? == Some(Value::Null))
        {
            return Ok(Some(json!({"kind":"selected", "model":selection["model"], "effort":selection["effort"]})));
        }
        let path = directory.join("started").join(format!("{key}.json"));
        if path.with_extension("done").try_exists()?
            && let Some(started) = files::read_mod::<Value>(&path)?.filter(|value| value["instance"] == host.instance)
        {
            if started["action"] == "reload-plugins"
                && files::host(directory)?
                    .is_some_and(|current| current.session == host.session && current.instance != host.instance)
            {
                return Ok(Some(done()));
            }
            if matches!(started["action"].as_str(), Some("exit" | "quit"))
                && directory.join("closed").join(format!("{}.json", host.instance)).try_exists()?
            {
                return Ok(Some(done()));
            }
        }
        Ok(None)
    }

    fn renamed(&self, name: &str) -> io::Result<bool> {
        let Some(boundary) = &self.transcript else { return Ok(false) };
        let mut file = File::open(&boundary.path)?;
        if file.metadata()?.len() < boundary.offset {
            return Ok(false);
        }
        file.seek(SeekFrom::Start(boundary.offset))?;
        let mut reader = BufReader::new(file);
        let mut title = None;
        let mut line = String::new();
        loop {
            line.clear();
            if reader.read_line(&mut line)? == 0 || !line.ends_with('\n') {
                break;
            }
            let row: Value = serde_json::from_str(&line).map_err(io::Error::other)?;
            if row["type"] == "custom-title" {
                title = row["customTitle"].as_str().map(str::to_owned);
            }
        }
        Ok(title.as_deref() == Some(name))
    }

    pub(super) fn executing(&mut self, claim: &fastctx_hosts::claude::control::Claim) -> io::Result<()> {
        if let Some(value) = &claim.evidence {
            *self = serde_json::from_value(value.clone()).map_err(io::Error::other)?;
        }
        self.process = claim.process.clone();
        Ok(())
    }

    pub(super) fn may_finish(&self) -> io::Result<bool> {
        if self.ended.as_ref().map(|path| path.try_exists()).transpose()?.unwrap_or(false) {
            return Ok(false);
        }
        let reconcilable = (self.compact && self.transcript.is_some())
            || rename(&self.control).is_some()
            || self.control["do"] == "model"
            || (self.control["do"] == "command"
                && matches!(self.control["name"].as_str(), Some("model" | "reload-plugins" | "exit" | "quit")));
        if !reconcilable {
            return Ok(false);
        }
        self.process.as_ref().map(fastctx_platform::is_alive).transpose().map(|alive| alive.unwrap_or(false))
    }
}

fn rename(control: &Value) -> Option<&str> {
    if control["do"] == "rename" {
        control["name"].as_str()
    } else if control["do"] == "command" && control["name"] == "rename" {
        control["args"].as_str().map(str::trim).filter(|name| !name.is_empty())
    } else {
        None
    }
}

pub(super) fn reply(value: Value) -> Result<Value, Failure> {
    if let Some(result) = value.get("ok") {
        return Ok(result.clone());
    }
    if let Some(error) = value.get("error").and_then(Value::as_str) {
        return Err(if value["uncertain"] == true {
            Failure::control_uncertain(error)
        } else if value["not_run"] == true {
            Failure::not_run(error)
        } else {
            Failure::new(error)
        });
    }
    Err(Failure::control_uncertain("Claude control reply has neither ok nor error; its outcome is unknown"))
}

pub(super) fn done() -> Value {
    json!({"kind":"done", "said":null})
}

pub(super) fn unknown(reason: &str) -> Failure {
    Failure::control_uncertain(format!("{reason}; the control's outcome is unknown and it will not be repeated"))
}
