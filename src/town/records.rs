//! A resident's records on the machine it lives on (`specs/world-town.md`, "世界的东西存在哪").
//!
//! `about` and `now` are JSON documents rewritten whole; `actions` is one line per action, tab
//! separated, so a model reading it with `fastctx view` sees plain text; `messages` is one JSON
//! object per line because a message spans lines. Nothing here is synced to other machines:
//! `fastctx w` asks the machine that holds them.

use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};

/// `<agent>-<4 hex>`, for example `codex-7f3a`.
pub(crate) fn new_resident_id(agent: &str, taken: impl Fn(&str) -> bool) -> Result<String, String> {
    let prefix = agent_prefix(agent);
    loop {
        let bytes = crate::world::crypto::random_bytes::<2>()?;
        let id = format!("{prefix}-{}", hex::encode(bytes));
        if !taken(&id) {
            return Ok(id);
        }
    }
}

/// The word an agent program goes by in resident ids.
pub(crate) fn agent_prefix(agent: &str) -> String {
    let lower = agent.to_ascii_lowercase();
    if lower.contains("claude") {
        "claude".to_string()
    } else if lower.contains("codex") {
        "codex".to_string()
    } else {
        let word: String = lower.chars().filter(char::is_ascii_alphanumeric).take(12).collect();
        if word.is_empty() { "agent".to_string() } else { word }
    }
}

/// Whether a text is shaped like a resident id.
pub(crate) fn is_resident_id(text: &str) -> bool {
    let Some((prefix, suffix)) = text.rsplit_once('-') else { return false };
    !prefix.is_empty()
        && prefix.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        && suffix.len() == 4
        && suffix.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// How a resident came to be.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum Birth {
    /// The user opened the session.
    User,
    /// Another resident asked for it; `label` is the three-to-eight-word task label. `machine`
    /// is where the requester lives when that is not this resident's machine.
    Colleague {
        requester: String,
        label: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        machine: Option<String>,
    },
}

/// Who it is and how it came to be; written once at birth, amended when the host reports more.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct About {
    pub(crate) id: String,
    /// Agent program as the host names itself (`clientInfo.name`), e.g. `claude-code`, `codex`.
    pub(crate) agent: String,
    #[serde(default)]
    pub(crate) agent_version: Option<String>,
    /// Model name and whether the host reported it (`host`) or only the resident did (`self`).
    #[serde(default)]
    pub(crate) model: Option<String>,
    #[serde(default)]
    pub(crate) model_source: Option<String>,
    pub(crate) machine: String,
    /// Where it runs: `vscode`, `app`, `cli`, `bg`, `world`, or what the host says.
    pub(crate) surface: String,
    pub(crate) birth: Birth,
    pub(crate) since: String,
    /// The host-side id the World uses to talk to it: a Codex thread id or a Claude session id.
    #[serde(default)]
    pub(crate) session: Option<String>,
    /// The town it is in; `None` while outside every town.
    #[serde(default)]
    pub(crate) town: Option<String>,
}

/// What a resident is doing right now. Only facts: what the host reported and what the World
/// saw its hands do.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct Now {
    /// `busy`, `idle`, `waiting`, `error`, `left`, or `unknown` when the host does not say.
    pub(crate) state: String,
    /// When `state` last changed.
    pub(crate) since: String,
    /// The command running right now, with the machine it runs on.
    #[serde(default)]
    pub(crate) running: Option<String>,
    /// The last action the World saw, already rendered as one line.
    #[serde(default)]
    pub(crate) last_action: Option<String>,
    #[serde(default)]
    pub(crate) last_action_at: Option<String>,
    /// Holding this machine's Computer Use desk for the current turn.
    #[serde(default)]
    pub(crate) desk: bool,
    /// The turn running now, as its host named it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) turn: Option<String>,
    /// Whose words went into the running turn: each is told how it ends. Kept here so that a
    /// node which restarts mid-turn still knows whom to tell.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) owed: Vec<crate::town::remote::Peer>,
    /// What went wrong with the session: the latest thing its host reported about the session
    /// rather than its work, or why its last turn did not finish. A turn that completes
    /// clears it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) trouble: Option<Trouble>,
}

/// Something wrong with a session, and when it was seen.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct Trouble {
    pub(crate) at: String,
    pub(crate) text: String,
}

/// One line of `messages`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct MessageLine {
    pub(crate) at: String,
    /// `asked` (the first request), `said` (someone spoke to it), `answered` (its turn ended),
    /// `delivered` (an answer reached its reader).
    pub(crate) kind: String,
    /// Who spoke, or who the answer was for.
    pub(crate) peer: String,
    #[serde(default)]
    pub(crate) turn: Option<String>,
    #[serde(default)]
    pub(crate) text: Option<String>,
}

/// Now, to the nanosecond: what a resident's arrival is stamped with, so the records it moves
/// into always outrank the ones it leaves, even within one second.
pub(crate) fn now_precise() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .expect("the current time formats as RFC 3339")
}

/// Orders records of one resident: those it has not left first, then the latest change. The
/// times are compared as moments, not as text, so precisions can mix.
pub(crate) fn currency(now: &Now) -> (bool, Option<time::OffsetDateTime>) {
    (now.state != "left", crate::world::parse_rfc3339(&now.since).ok())
}

/// Where a resident's records are on a machine: in its town's directory, or outside every
/// town. A resident living elsewhere has the same directory here for what it runs here.
pub(crate) fn resident_dir(world_dir: &Path, town: Option<&str>, id: &str) -> PathBuf {
    match town {
        Some(town) => world_dir.join("towns").join(town).join("residents").join(id),
        None => world_dir.join("outside").join("residents").join(id),
    }
}

/// The directory of one resident.
#[derive(Clone, Debug)]
pub(crate) struct Records {
    pub(crate) dir: PathBuf,
}

impl Records {
    pub(crate) fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    pub(crate) fn create(&self) -> Result<(), String> {
        std::fs::create_dir_all(self.dir.join("jobs"))
            .and_then(|()| std::fs::create_dir_all(self.dir.join("out")))
            .map_err(|error| format!("Cannot create {}: {error}", crate::paths::display_path(&self.dir)))
    }

    pub(crate) fn about(&self) -> Result<Option<About>, String> {
        read_json(&self.dir.join("about"))
    }

    pub(crate) fn write_about(&self, about: &About) -> Result<(), String> {
        write_json(&self.dir.join("about"), about)
    }

    pub(crate) fn now(&self) -> Result<Option<Now>, String> {
        read_json(&self.dir.join("now"))
    }

    pub(crate) fn write_now(&self, now: &Now) -> Result<(), String> {
        write_json(&self.dir.join("now"), now)
    }

    /// Changes part of `now` and keeps the rest. Every such change in this process goes through
    /// here one at a time, so two made at once (a turn ending while an action is recorded)
    /// never undo each other.
    pub(crate) fn update_now(&self, change: impl FnOnce(&mut Now)) -> Result<(), String> {
        static CHANGING: parking_lot::Mutex<()> = parking_lot::Mutex::new(());
        let _changing = CHANGING.lock();
        let mut now = self.now()?.unwrap_or_default();
        change(&mut now);
        self.write_now(&now)
    }

    /// Appends one action: when, where, which hand, what, and the result.
    pub(crate) fn append_action(&self, at: &str, machine: &str, hand: &str, what: &str, result: &str) -> Result<String, String> {
        let line = format!("{at}\t{machine}\t{hand}\t{}\t{}", one_line(what), one_line(result));
        append_line(&self.dir.join("actions"), &line)?;
        Ok(line)
    }

    pub(crate) fn append_message(&self, message: &MessageLine) -> Result<(), String> {
        let line = serde_json::to_string(message).map_err(|error| format!("Cannot encode a message line: {error}"))?;
        append_line(&self.dir.join("messages"), &line)
    }

    /// Every message line, and how many lines could not be read: a write cut short leaves a
    /// partial last line, which must not hide the lines before it.
    pub(crate) fn messages(&self) -> Result<(Vec<MessageLine>, usize), String> {
        let path = self.dir.join("messages");
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok((Vec::new(), 0)),
            Err(error) => return Err(format!("Cannot read {}: {error}", crate::paths::display_path(&path))),
        };
        // Line by line, so a line cut off mid-character costs only that line.
        let mut lines = Vec::new();
        let mut damaged = 0;
        for line in bytes.split(|byte| *byte == b'\n').filter(|line| !line.trim_ascii().is_empty()) {
            match serde_json::from_slice(line) {
                Ok(message) => lines.push(message),
                Err(_) => damaged += 1,
            }
        }
        Ok((lines, damaged))
    }

    /// The last `count` action lines and how many there are in all.
    pub(crate) fn recent_actions(&self, count: usize) -> Result<(Vec<String>, usize), String> {
        let path = self.dir.join("actions");
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok((Vec::new(), 0)),
            Err(error) => return Err(format!("Cannot read {}: {error}", crate::paths::display_path(&path))),
        };
        let lines: Vec<&str> = text.lines().filter(|line| !line.is_empty()).collect();
        let total = lines.len();
        let start = total.saturating_sub(count);
        Ok((lines[start..].iter().map(|line| (*line).to_string()).collect(), total))
    }

    /// The colleagues this resident asked for, each with the machine it lives on, as its
    /// `ask_colleague` actions recorded them (`machine`, then the colleague's id as the result).
    pub(crate) fn colleagues_asked(&self) -> Result<Vec<(String, String)>, String> {
        let path = self.dir.join("actions");
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(format!("Cannot read {}: {error}", crate::paths::display_path(&path))),
        };
        Ok(text
            .lines()
            .filter_map(|line| {
                let fields: Vec<&str> = line.splitn(5, '\t').collect();
                match fields.as_slice() {
                    [_, machine, "ask_colleague", _, id] if is_resident_id(id) => Some((machine.to_string(), id.to_string())),
                    _ => None,
                }
            })
            .collect())
    }
}

/// Collapses newlines and tabs so an action stays one line with five fields.
fn one_line(text: &str) -> String {
    text.split(['\n', '\r', '\t']).filter(|part| !part.is_empty()).collect::<Vec<_>>().join(" ")
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<Option<T>, String> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| format!("Cannot parse {}: {error}", crate::paths::display_path(path))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("Cannot read {}: {error}", crate::paths::display_path(path))),
    }
}

/// Replaces a record file whole. Several writers may replace the same file at once (a turn
/// ending while an action is recorded), so each writes its own temporary file and the last
/// rename wins; none fails because another one was writing.
fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|error| format!("Cannot encode {}: {error}", crate::paths::display_path(path)))?;
    crate::atomic_file::publish(path, &bytes)
}

fn append_line(path: &Path, line: &str) -> Result<(), String> {
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| format!("Cannot open {}: {error}", crate::paths::display_path(path)))?;
    file.write_all(format!("{line}\n").as_bytes())
        .map_err(|error| format!("Cannot append to {}: {error}", crate::paths::display_path(path)))
}
