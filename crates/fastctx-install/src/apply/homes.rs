//! Ownership is bound to a host and its resolved home, independently of the
//! environment that selected it. The shared journal still commits every edit
//! together with the complete registry; v1 receipts retain their original paths.

use sha2::{Digest, Sha256};

use super::*;
use crate::transaction::CreatedDirectory;

/// The host whose configuration home is selected; executable discovery is separate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Host {
    Codex,
    Claude,
}

impl Host {
    /// Selects this host alone for an apply or unapply.
    pub fn only(self) -> Hosts {
        Hosts { codex: self == Self::Codex, claude: self == Self::Claude }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Home {
    host: Host,
    home: PathBuf,
    receipt: Record,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Records {
    version: u32,
    homes: Vec<Home>,
    created: Created,
}

pub(super) fn resolve(home: &Path) -> Result<PathBuf, Error> {
    if home.as_os_str().is_empty() {
        return Err(Error::Io("host home must not be empty".into()));
    }
    let (path, _) = crate::snapshot::resolve(&home.join(".fastctx-home"))
        .map_err(|error| FileError::io(home, "resolve host home", error))?;
    Ok(path.parent().expect("resolved file has a parent").to_owned())
}

fn key(home: &Path) -> Result<String, Error> {
    let home = resolve(home)?;
    identity(&home)
}

fn identity(home: &Path) -> Result<String, Error> {
    let text = home.to_str().ok_or_else(|| Error::Io("host home must be a UTF-8 path".into()))?;
    Ok(if cfg!(windows) {
        text.replace('\\', "/").trim_start_matches("//?/").to_ascii_lowercase()
    } else {
        text.to_owned()
    })
}

fn parent(path: &Path) -> Result<PathBuf, Error> {
    resolve(path.parent().ok_or_else(|| Error::Io(format!("{} has no home directory", path.display())))?)
}

impl Records {
    pub(super) fn read(snapshot: &Snapshot, places: &Places) -> Result<Self, Error> {
        #[derive(Deserialize)]
        struct Version {
            version: u32,
        }
        let empty = || Self { version: 2, homes: vec![], created: Created::default() };
        let Some(bytes) = snapshot.bytes() else { return Ok(empty()) };
        let version: Version = serde_json::from_slice(bytes)
            .map_err(|error| Error::Io(format!("reading {}: {error}", snapshot.path().display())))?;
        if version.version == 1 {
            let old = Record::read(snapshot)?;
            let mut records = empty();
            records.created = old.created.clone();
            if let Some(receipt) = &old.codex {
                records.put(Host::Codex, parent(receipt.config_path())?, &old);
            }
            if old.claude_md.is_some() || old.settings.is_some() || old.claude_tools.is_some() {
                let home = old
                    .claude_md
                    .as_ref()
                    .map(|receipt| Path::new(&receipt.path))
                    .or_else(|| old.claude_tools.as_ref().map(|receipt| receipt.settings_path()))
                    .map(parent)
                    .transpose()?
                    .or(plugin_home(places, &old)?);
                let home = home.ok_or_else(|| Error::Io("old Claude apply record has no home path".into()))?;
                records.put(Host::Claude, home, &old);
            }
            return Ok(records);
        }
        if version.version != 2 {
            return Err(Error::Io(format!(
                "{}: unsupported apply record version {}",
                snapshot.path().display(),
                version.version
            )));
        }
        let records: Self = serde_json::from_slice(bytes)
            .map_err(|error| Error::Io(format!("reading {}: {error}", snapshot.path().display())))?;
        for (index, home) in records.homes.iter().enumerate() {
            let identity = identity(&home.home)?;
            for previous in &records.homes[..index] {
                if home.host == previous.host && identity == self::identity(&previous.home)? {
                    return Err(Error::Io(format!("duplicate apply home: {}", home.home.display())));
                }
            }
        }
        Ok(records)
    }

    fn find(&self, host: Host, home: &Path) -> Result<Option<&Record>, Error> {
        let identity = key(home)?;
        for entry in &self.homes {
            if entry.host == host && self::identity(&entry.home)? == identity {
                return Ok(Some(&entry.receipt));
            }
        }
        Ok(None)
    }

    pub(super) fn selected(&self, places: &Places, hosts: Hosts) -> Result<Record, Error> {
        let codex = if hosts.codex { self.find(Host::Codex, &places.codex_home)? } else { None };
        let claude = if hosts.claude { self.find(Host::Claude, &places.claude_home)? } else { None };
        Ok(Record {
            version: RECORD_VERSION,
            codex: codex.and_then(|record| record.codex.clone()),
            claude_md: claude.and_then(|record| record.claude_md.clone()),
            settings: claude.and_then(|record| record.settings.clone()),
            claude_tools: claude.and_then(|record| record.claude_tools.clone()),
            settings_created: claude.is_some_and(|record| record.settings_created),
            created: self.created.clone(),
        })
    }

    fn put(&mut self, host: Host, home: PathBuf, selected: &Record) {
        let mut receipt = selected.clone();
        receipt.created = Created::default();
        match host {
            Host::Codex => {
                receipt.claude_md = None;
                receipt.settings = None;
                receipt.claude_tools = None;
                receipt.settings_created = false;
            }
            Host::Claude => receipt.codex = None,
        }
        if !receipt.holds_nothing() {
            self.homes.push(Home { host, home, receipt });
        }
    }

    pub(super) fn update(
        &mut self,
        places: &Places,
        hosts: Hosts,
        selected: &Record,
    ) -> Result<Vec<CreatedDirectory>, Error> {
        let targets: Vec<_> =
            [(Host::Codex, hosts.codex, &places.codex_home), (Host::Claude, hosts.claude, &places.claude_home)]
                .into_iter()
                .filter(|(_, selected, _)| *selected)
                .map(|(host, _, home)| Ok((host, resolve(home)?, key(home)?)))
                .collect::<Result<_, Error>>()?;
        let mut kept = Vec::new();
        for entry in self.homes.drain(..) {
            let identity = identity(&entry.home)?;
            if !targets.iter().any(|(host, _, target)| *host == entry.host && *target == identity) {
                kept.push(entry);
            }
        }
        self.homes = kept;
        for (host, home, _) in &targets {
            self.put(*host, home.clone(), selected);
        }
        self.created = selected.created.clone();
        let root = resolve(&places.root)?;
        let mut cleanup = Vec::new();
        let mut directories = Vec::new();
        for directory in self.created.directories.drain(..) {
            let path = resolve(&directory.path)?;
            let was_used = targets.iter().any(|(_, home, _)| home.starts_with(&path));
            let still_used = self.homes.iter().any(|entry| {
                entry.home.starts_with(&path)
                    && (entry.host != Host::Codex || entry.receipt.codex.as_ref().is_some_and(codex::Receipt::has_work))
            });
            if was_used && !still_used && !path.starts_with(&root) && !root.starts_with(&path) {
                cleanup.push(directory);
            } else {
                directories.push(directory);
            }
        }
        self.created.directories = directories;
        Ok(cleanup)
    }

    pub(super) fn is_empty(&self) -> bool {
        self.homes.is_empty()
    }
}

fn plugin_home(places: &Places, record: &Record) -> Result<Option<PathBuf>, Error> {
    let plugin = load_plugin_record(&places.root.join("applied-claude-plugin.json"))?;
    plugin
        .and_then(|plugin| plugin.settings_path)
        .or_else(|| record.claude_md.as_ref().map(|receipt| Path::new(&receipt.path).with_file_name("settings.json")))
        .as_deref()
        .map(parent)
        .transpose()
}

/// The old filename stays in use for its recorded home until withdrawal.
/// No eager migration can detach an unfinished CLI operation from its settings.
pub(super) fn plugin_record(places: &Places) -> Result<PathBuf, Error> {
    let old = places.root.join("applied-claude-plugin.json");
    if old.try_exists().map_err(io("checking old plugin record"))? {
        let home = old_plugin_home(places)?;
        if key(&home)? == key(&places.claude_home)? {
            return Ok(old);
        }
    }
    let digest = Sha256::digest(key(&places.claude_home)?.as_bytes());
    Ok(places.root.join(format!("applied-claude-plugin-{digest:x}.json")))
}

pub(super) fn old_plugin_home(places: &Places) -> Result<PathBuf, Error> {
    let snapshot = Snapshot::read(places.record())?;
    let record = match snapshot.bytes() {
        Some(bytes)
            if serde_json::from_slice::<serde_json::Value>(bytes)
                .map_err(|error| Error::Io(format!("reading {}: {error}", snapshot.path().display())))?
                .get("version")
                == Some(&1.into()) =>
        {
            Record::read(&snapshot)?
        }
        _ => Record::default(),
    };
    match plugin_home(places, &record)? {
        Some(home) => Ok(home),
        None => resolve(&default_claude_home()?),
    }
}

fn default_claude_home() -> Result<PathBuf, Error> {
    match std::env::var_os("CLAUDE_CONFIG_DIR") {
        Some(home) if !home.is_empty() => Ok(home.into()),
        _ => Ok(fastctx_platform::user_home().map_err(io("finding user's home"))?.join(".claude")),
    }
}

pub(super) fn legacy(places: &Places) -> Result<Option<(Snapshot, legacy::Receipt)>, Error> {
    let file = Snapshot::read(places.root.join("config.toml"))?;
    let receipt = file.bytes().map(legacy::Receipt::from_bytes).transpose()?.flatten();
    match receipt {
        Some(receipt) if key(&parent(Path::new(receipt.config_path()))?)? == key(&places.codex_home)? => {
            Ok(Some((file, receipt)))
        }
        _ => Ok(None),
    }
}
