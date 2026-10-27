//! Files for the Claude control bridge. Only public host hooks and methods
//! create requests; their payload and the person's answer stay in host form.

use std::io;
use std::path::{Path, PathBuf};

use fastctx_platform::{Permissions, ensure_private_dir, publish, publish_new};
use fastctx_types::ProcessIdentity;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The background-safe command/argument/result contract of this mod.
pub const COMMAND_CONTRACT: u32 = 2;

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Host {
    pub session: String,
    pub instance: String,
    /// The process stream owned by this mod activation, not the Claude process.
    pub process: ProcessIdentity,
    /// Old activations may still report the unfiltered terminal menu until
    /// hot reload. They remain request bridges, but do not offer controls.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_contract: Option<u32>,
    /// Unclaimed controls can be taken by a replacement activation under a durable claim.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handoff_contract: Option<u32>,
    /// The settings flag observed by this activation, retained across cold resume.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assisted: Option<bool>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Claim {
    pub host: Host,
    /// The activation closed before entering the control's implementation.
    #[serde(default)]
    pub released: bool,
    pub process: Option<ProcessIdentity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Question {
    pub key: String,
    pub host: Host,
    pub turn: Option<String>,
    pub request: String,
    pub params: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Waiting {
    #[serde(flatten)]
    pub question: Question,
    pub process: ProcessIdentity,
    pub at: Timestamp,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Answer {
    Answer { answer: Value },
    Pass { reason: String },
}

impl Question {
    /// Opaque to clients. Both the mod activation and the helper's unique
    /// request key participate, so a restart cannot reuse an answer address.
    pub fn address(&self) -> Value {
        serde_json::json!(["claude", self.host.session, self.host.instance, self.key])
    }

    pub fn directory(&self, control: &Path) -> io::Result<PathBuf> {
        for name in [&self.host.instance, &self.key] {
            if name.is_empty() || !name.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-') {
                return Err(io::Error::other("invalid Claude request path component"));
            }
        }
        Ok(control.join("requests").join(format!("{}-{}", self.host.instance, self.key)))
    }
}

pub fn directory(world: &Path, session: &str) -> PathBuf {
    super::exchange::sessions(world).join(session).join("control")
}

/// The directories of the requests a session's helpers wrote under its
/// control directory `control`. The mod makes `requests` by writing a file
/// in it (`.created`), which is no request.
pub fn requests(control: &Path) -> io::Result<Vec<PathBuf>> {
    let entries = match std::fs::read_dir(control.join("requests")) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut requests = Vec::new();
    for entry in entries {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            requests.push(entry.path());
        }
    }
    requests.sort();
    Ok(requests)
}

pub fn read<T: serde::de::DeserializeOwned>(path: &Path) -> io::Result<Option<T>> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map(Some).map_err(io::Error::other),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

pub fn write(path: &Path, value: &impl Serialize) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        ensure_private_dir(parent)?;
    }
    publish(path, &serde_json::to_vec(value).map_err(io::Error::other)?, Permissions::Private)
}

/// The mod's fs API allows 4 MiB per operation. Larger control envelopes
/// use the same numbered UTF-8 parts as the transcript event exchange.
pub fn write_message(path: &Path, value: &impl Serialize) -> io::Result<()> {
    const FILE_BYTES: usize = 4 * 1024 * 1024;
    let text = serde_json::to_string(value).map_err(io::Error::other)?;
    if text.len() <= FILE_BYTES {
        return write(path, value);
    }
    let mut rest = text.as_str();
    let mut count = 0;
    while !rest.is_empty() {
        let mut end = rest.len().min(FILE_BYTES);
        while !rest.is_char_boundary(end) {
            end -= 1;
        }
        let part = path.with_extension("parts").join(count.to_string());
        ensure_private_dir(part.parent().expect("numbered part"))?;
        publish(&part, &rest.as_bytes()[..end], Permissions::Private)?;
        count += 1;
        rest = &rest[end..];
    }
    write(path, &serde_json::json!({"parts":count}))
}

pub fn read_message(path: &Path) -> io::Result<Value> {
    let value = read::<Value>(path)?.ok_or_else(|| io::Error::other("Claude marked a missing reply complete"))?;
    let Some(count) = value.get("parts").and_then(Value::as_u64) else { return Ok(value) };
    let mut body = Vec::new();
    for index in 0..count {
        body.extend(std::fs::read(path.with_extension("parts").join(index.to_string()))?);
    }
    serde_json::from_slice(&body).map_err(io::Error::other)
}

pub fn answer(directory: &Path, answer: &Answer) -> io::Result<()> {
    publish_new(
        &directory.join("answer.json"),
        &serde_json::to_vec(answer).map_err(io::Error::other)?,
        Permissions::Private,
    )
}

pub fn host(directory: &Path) -> io::Result<Option<Host>> {
    // The mod's write is not atomic. The completed registration is checked
    // again by identity before executing or answering anything.
    if !directory.join("host.done").exists() {
        return Ok(None);
    }
    read_mod(&directory.join("host.json"))
}

/// A mod fs.write may be observed before its final byte. Only an EOF is
/// incomplete; malformed complete data and filesystem errors remain errors.
pub fn read_mod<T: serde::de::DeserializeOwned>(path: &Path) -> io::Result<Option<T>> {
    match std::fs::read(path) {
        Ok(bytes) => match serde_json::from_slice(&bytes) {
            Ok(host) => Ok(Some(host)),
            Err(error) if error.is_eof() => Ok(None),
            Err(error) => Err(io::Error::other(error)),
        },
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

pub fn active(directory: &Path, host: &Host) -> io::Result<bool> {
    Ok(self::host(directory)?.as_ref() == Some(host)
        && !directory.join("closed").join(format!("{}.json", host.instance)).exists()
        && fastctx_platform::is_alive(&host.process)?)
}

/// The public return types are checked before a reply is committed. The
/// host still validates tool result schemas and permission policy itself.
pub fn validate_answer(question: &Question, answer: &Value) -> Result<(), String> {
    let object = answer.as_object().ok_or("a Claude answer must be an object")?;
    match question.request.as_str() {
        "tool.check" => {
            if object.keys().any(|key| !["decision", "reason"].contains(&key.as_str()))
                || !matches!(object.get("decision").and_then(Value::as_str), Some("allow" | "deny"))
                || object.get("reason").is_some_and(|value| !value.is_string())
            {
                return Err("tool.check takes {decision:allow|deny,reason?:string}".into());
            }
            if object["decision"] == "allow" && question.params["ceiling"] == "ask" {
                return Err("this tool requires the host's own approval under the organization's ceiling".into());
            }
        }
        "classic.PermissionRequest" => {
            if object.keys().any(|key| key != "decision") {
                return Err("PermissionRequest takes {decision:{behavior,...}}".into());
            }
            let decision = answer.get("decision").and_then(Value::as_object).ok_or("decision is required")?;
            match decision.get("behavior").and_then(Value::as_str) {
                Some("allow") => {
                    if decision
                        .keys()
                        .any(|key| !["behavior", "updatedInput", "updatedPermissions"].contains(&key.as_str()))
                        || decision.get("updatedInput").is_some_and(|value| !value.is_object())
                        || decision.get("updatedPermissions").is_some_and(|value| !value.is_array())
                    {
                        return Err("invalid allow decision".into());
                    }
                }
                Some("deny") => {
                    if decision.keys().any(|key| !["behavior", "message", "interrupt"].contains(&key.as_str()))
                        || decision.get("message").is_some_and(|value| !value.is_string())
                        || decision.get("interrupt").is_some_and(|value| value != true)
                    {
                        return Err("invalid deny decision".into());
                    }
                }
                _ => return Err("permission behavior must be allow or deny".into()),
            }
        }
        "Elicitation" => {
            if !matches!(object.get("action").and_then(Value::as_str), Some("accept" | "decline" | "cancel"))
                || object.keys().any(|key| !["action", "content"].contains(&key.as_str()))
                || object.get("content").is_some_and(|value| !value.is_object())
            {
                return Err("Elicitation takes {action:accept|decline|cancel,content?:object}".into());
            }
        }
        _ => return Err("this Claude request has no public answer contract".into()),
    }
    Ok(())
}
