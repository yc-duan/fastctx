//! `fastctx apply` and `fastctx unapply`: the World's tools put into the
//! sessions the user opens on this machine, and taken out again, as one
//! change (`world-run.md`, 模型怎样知道).
//!
//! An apply places fastctx where the hosts start it from, works out every
//! host file's next bytes from what is on disk now and from the record of
//! what an earlier apply wrote, and commits them together with that record.
//! Claude Code's plugin goes in through Claude Code's own `claude plugin`
//! commands before the files are read, since those commands write Claude's
//! settings; when the files cannot be committed afterwards, what this apply
//! added to Claude is taken back. Each file is either as it was or as it is
//! meant to be; a file someone changed after it was read is left as they
//! left it and named. Run again after an interruption, an apply finishes
//! what it started, since it starts from what is there.
//!
//! An unapply takes out what the record says an apply put in, and leaves
//! what the user changed since, saying where. It leaves fastctx's own copy,
//! which the node's service also starts from.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use fastctx_platform::{FileLock, Permissions};
use fastctx_types::layout::WorldDir;
use serde::{Deserialize, Serialize};

use crate::briefing::{self, Writer};
use crate::claude::plugins::{self, PluginApply, PluginCommand, PluginFailure, PluginHost, PluginReceipt};
use crate::claude::settings::{self, SettingsReceipt};
use crate::codex::{self, Approval, Ownership};
use crate::plan::{ClaudePlan, CodexPlan};
use crate::transaction::{Created, FileChange, FileError, Snapshot, Transaction};
use crate::{File, Notice, executable, guidance, legacy};

/// Where the things an apply touches are, for this user on this machine.
#[derive(Clone, Debug)]
pub struct Places {
    /// fastctx's own directory: `~/.fastctx`, or `FASTCTX_HOME`.
    pub root: PathBuf,
    /// The World's directory, where the node keeps Claude's plugin.
    pub world: PathBuf,
    /// Codex's directory: `~/.codex`, or `CODEX_HOME`.
    pub codex_home: PathBuf,
    /// Claude Code's directory: `~/.claude`, or `CLAUDE_CONFIG_DIR`.
    pub claude_home: PathBuf,
    /// The `claude` command, when it is on `PATH`.
    pub claude: Option<PathBuf>,
    /// The fastctx that runs now, which an apply copies into place.
    pub fastctx: PathBuf,
}

impl Places {
    /// This user's, found as fastctx and each host find them.
    pub fn here() -> io::Result<Self> {
        let root = fastctx_platform::fastctx_root()?;
        let home = fastctx_platform::user_home()?;
        let environment: Vec<(OsString, OsString)> = std::env::vars_os().collect();
        let claude_home = match std::env::var_os("CLAUDE_CONFIG_DIR") {
            Some(directory) if !directory.is_empty() => PathBuf::from(directory),
            _ => home.join(".claude"),
        };
        Ok(Self {
            world: WorldDir::under(&root).path().to_path_buf(),
            codex_home: fastctx_hosts::codex::launch::codex_home(&home, &environment),
            claude_home,
            claude: crate::claude::find(std::env::var_os("PATH").as_ref()),
            fastctx: std::env::current_exe()?,
            root,
        })
    }

    /// The stable copy of fastctx the hosts start.
    pub fn installed(&self) -> PathBuf {
        executable::installed_path(&self.root)
    }

    fn record(&self) -> PathBuf {
        self.root.join("applied.json")
    }

    /// Claude's plugin commands are checkpointed as they run, before the
    /// files are committed, so their record is a file of its own.
    fn plugin_record(&self) -> PathBuf {
        self.root.join("applied-claude-plugin.json")
    }

    fn lock(&self) -> PathBuf {
        self.root.join("apply.lock")
    }
}

/// Which hosts an apply or unapply is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hosts {
    pub codex: bool,
    pub claude: bool,
}

impl Hosts {
    /// The hosts installed here: Codex when its directory exists, Claude
    /// Code when its command is found.
    pub fn present(places: &Places) -> Self {
        Self { codex: places.codex_home.is_dir(), claude: places.claude.is_some() }
    }
}

/// What an apply or unapply did, for the person who asked.
#[derive(Debug, Default)]
pub struct Report {
    /// Files written or removed.
    pub changed: Vec<PathBuf>,
    /// The `claude` commands that changed Claude Code, as run.
    pub commands: Vec<String>,
    /// Places left alone, and why.
    pub notices: Vec<Notice>,
    /// What was done in full but left something behind, such as an old copy
    /// of fastctx still running.
    pub leftovers: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Io(String),
    #[error(transparent)]
    Edit(#[from] crate::Error),
    #[error("Claude Code: {0}")]
    Claude(String),
    #[error("Claude Code's plugin: {0}")]
    Plugin(#[from] PluginFailure),
    #[error("nothing was changed: {0}")]
    Files(String),
    #[error("the claude command is not on PATH")]
    NoClaude,
}

impl From<FileError> for Error {
    fn from(error: FileError) -> Self {
        Self::Io(error.to_string())
    }
}

/// What an apply wrote, so that an unapply takes out exactly that.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u32,
    codex: Option<codex::Receipt>,
    claude_md: Option<guidance::Receipt>,
    settings: Option<SettingsReceipt>,
    /// Files and directories an apply created, which an unapply may remove
    /// when they are empty or unchanged.
    created: Created,
}

const RECORD_VERSION: u32 = 1;

impl Record {
    fn read(snapshot: &Snapshot) -> Result<Self, Error> {
        match snapshot.bytes() {
            None => Ok(Self { version: RECORD_VERSION, ..Self::default() }),
            Some(bytes) => serde_json::from_slice(bytes)
                .map_err(|error| Error::Io(format!("reading {}: {error}", snapshot.path().display()))),
        }
    }

    fn holds_nothing(&self) -> bool {
        self.codex.is_none() && self.claude_md.is_none() && self.settings.is_none()
    }
}

/// Puts the World's tools into the sessions `hosts` open from now on.
/// `approval` says whether Codex's shared output limit may be raised when
/// the user set it lower than the World needs.
pub fn apply(places: &Places, hosts: Hosts, approval: Approval) -> Result<Report, Error> {
    fastctx_platform::ensure_private_dir(&places.root).map_err(io("making fastctx's directory"))?;
    let _lock = FileLock::acquire(&places.lock()).map_err(io("taking the apply lock"))?;
    let record_file = Snapshot::read(places.record())?;
    let mut record = Record::read(&record_file)?;
    let installed = places.installed();
    let mut changes = Vec::new();
    let mut report = Report::default();

    let copy = Snapshot::read(&installed)?;
    if !same_executable(&places.fastctx, &copy)? {
        changes.push(FileChange::copy_executable(copy, &places.fastctx)?);
    }

    if hosts.codex {
        let config = Snapshot::read(places.codex_home.join("config.toml"))?;
        let agents = Snapshot::read(places.codex_home.join("AGENTS.md"))?;
        // The old fastctx (0.1.0 to 0.2.6) kept its record in its settings.
        let old = match &record.codex {
            Some(_) => None,
            None => read_legacy(&places.root)?,
        };
        let ownership = match (&record.codex, &old) {
            (Some(receipt), _) => Ownership::Current(receipt),
            (None, Some(receipt)) => Ownership::Legacy(receipt),
            (None, None) => Ownership::None,
        };
        let (config_path, agents_path) = (text(config.path()), text(agents.path()));
        let applied = codex::apply(
            File { path: &config_path, bytes: config.bytes() },
            File { path: &agents_path, bytes: agents.bytes() },
            &CodexPlan::new(&installed),
            &briefing::guidance(Writer::ApplyPatch),
            ownership,
            approval,
        )?;
        change(&mut changes, config, Some(applied.config));
        change(&mut changes, agents, Some(applied.agents));
        record.codex = Some(applied.receipt);
        report.notices.extend(applied.notices);
    }

    let mut plugin = None;
    if hosts.claude {
        let claude = places.claude.clone().ok_or(Error::NoClaude)?;
        fastctx_hosts::claude::write_plugin(&places.world, &installed).map_err(io("writing the World's plugin"))?;
        let plan = ClaudePlan::new(&places.world);
        let mut host = Cli::new(claude, places.plugin_record());
        let previous = read_plugin_record(&places.plugin_record())?;
        let before = plugins::snapshot(&mut host).map_err(Error::Claude)?;
        let planned = PluginApply::prepare(&plan, before, previous.as_ref()).map_err(Error::Claude)?;
        let added = planned.execute(&mut host)?;
        report.commands.append(&mut host.ran);

        // Read only now: the plugin commands write Claude's settings.
        let settings_file = Snapshot::read(places.claude_home.join("settings.json"))?;
        let edit = settings::apply(settings_file.bytes(), &plan, record.settings.as_ref()).map_err(Error::Claude)?;
        change(&mut changes, settings_file, edit.bytes);
        record.settings = edit.receipt;

        let claude_md = Snapshot::read(places.claude_home.join("CLAUDE.md"))?;
        let path = text(claude_md.path());
        let applied = guidance::apply(
            File { path: &path, bytes: claude_md.bytes() },
            &briefing::guidance(Writer::Edit),
            record.claude_md.as_ref(),
        )?;
        change(&mut changes, claude_md, Some(applied.bytes));
        record.claude_md = Some(applied.receipt);
        plugin = Some((planned, added, host));
    }

    record.version = RECORD_VERSION;
    // fastctx's own copy, and the directory it lives in, belong to fastctx,
    // not to what an unapply takes out.
    let bin = installed.parent().map(Path::to_path_buf);
    let committed = Transaction::new(changes).commit_with_record(record_file, |created| {
        let mut record = record.clone();
        record.created.files.extend(created.files.iter().filter(|file| !same_path(file, &installed)).cloned());
        record.created.directories.extend(
            created
                .directories
                .iter()
                .filter(|directory| bin.as_deref().is_none_or(|bin| !same_path(&directory.path, bin)))
                .cloned(),
        );
        serde_json::to_vec_pretty(&record).map_err(io::Error::other)
    });
    match committed {
        Ok(receipt) => {
            report.changed = receipt.changed;
            report.leftovers.extend(receipt.cleanup.iter().map(ToString::to_string));
            report.leftovers.extend(executable::cleanup_retired(&installed).iter().map(ToString::to_string));
            Ok(report)
        }
        Err(failure) => {
            // The files are as they were; what this apply added to Claude
            // goes too, so that nothing is half in.
            let mut said = failure.to_string();
            if let Some((planned, added, mut host)) = plugin
                && let Err(error) = planned.rollback(&mut host, &added)
            {
                said.push_str(&format!("\nand what was added to Claude Code could not be taken back: {error}"));
            }
            Err(Error::Files(said))
        }
    }
}

/// Takes out of `hosts` what an apply put in, leaving what the user changed
/// since.
pub fn unapply(places: &Places, hosts: Hosts) -> Result<Report, Error> {
    fastctx_platform::ensure_private_dir(&places.root).map_err(io("making fastctx's directory"))?;
    let _lock = FileLock::acquire(&places.lock()).map_err(io("taking the apply lock"))?;
    let record_file = Snapshot::read(places.record())?;
    let mut record = Record::read(&record_file)?;
    let mut changes = Vec::new();
    let mut report = Report::default();

    if hosts.codex
        && let Some(receipt) = record.codex.take()
    {
        let config = Snapshot::read(places.codex_home.join("config.toml"))?;
        let agents = Snapshot::read(places.codex_home.join("AGENTS.md"))?;
        let (config_path, agents_path) = (text(config.path()), text(agents.path()));
        let taken = codex::unapply(
            File { path: &config_path, bytes: config.bytes() },
            File { path: &agents_path, bytes: agents.bytes() },
            Some(&receipt),
        )?;
        change(&mut changes, config, taken.config);
        change(&mut changes, agents, taken.agents);
        record.codex = taken.receipt;
        report.notices.extend(taken.retained);
    }

    let mut plugin_done = false;
    if hosts.claude {
        if let Some(previous) = read_plugin_record(&places.plugin_record())? {
            let claude = places.claude.clone().ok_or(Error::NoClaude)?;
            let mut host = Cli::new(claude, places.plugin_record());
            let remaining = plugins::unapply(&mut host, &previous)?;
            report.commands.append(&mut host.ran);
            plugin_done = !remaining.has_work();
        }
        let settings_file = Snapshot::read(places.claude_home.join("settings.json"))?;
        if let Some(receipt) = record.settings.take() {
            let edit = settings::unapply(settings_file.bytes(), Some(&receipt)).map_err(Error::Claude)?;
            change(&mut changes, settings_file, edit.bytes);
            record.settings = edit.receipt;
        }
        if let Some(receipt) = record.claude_md.take() {
            let claude_md = Snapshot::read(places.claude_home.join("CLAUDE.md"))?;
            let path = text(claude_md.path());
            let taken = guidance::unapply(File { path: &path, bytes: claude_md.bytes() }, Some(&receipt))?;
            change(&mut changes, claude_md, taken.bytes);
            record.claude_md = taken.receipt;
            report.notices.extend(taken.retained);
        }
    }

    let committed = if record.holds_nothing() {
        if record_file.exists() {
            changes.push(FileChange::delete(record_file));
        }
        Transaction::new(changes).commit()
    } else {
        Transaction::new(changes)
            .commit_with_record(record_file, |_| serde_json::to_vec_pretty(&record).map_err(io::Error::other))
    };
    let receipt = committed.map_err(|failure| Error::Files(failure.to_string()))?;
    report.changed = receipt.changed;
    report.leftovers.extend(receipt.cleanup.iter().map(ToString::to_string));
    if plugin_done {
        match std::fs::remove_file(places.plugin_record()) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => report.leftovers.push(format!("{}: {error}", places.plugin_record().display())),
        }
    }
    if record.holds_nothing() {
        let created = std::mem::take(&mut record.created);
        report
            .leftovers
            .extend(crate::transaction::cleanup_directories(&created.directories).iter().map(ToString::to_string));
    }
    Ok(report)
}

/// Runs the public `claude` command for the plugin engine, and keeps each
/// checkpoint it hands over before the next command runs.
struct Cli {
    claude: PathBuf,
    record: PathBuf,
    /// The commands that changed Claude, as run.
    ran: Vec<String>,
}

impl Cli {
    fn new(claude: PathBuf, record: PathBuf) -> Self {
        Self { claude, record, ran: Vec::new() }
    }
}

impl PluginHost for Cli {
    fn run(&mut self, command: &PluginCommand) -> Result<Vec<u8>, String> {
        let output = Command::new(&self.claude)
            .args(&command.args)
            .stdin(Stdio::null())
            .output()
            .map_err(|error| format!("running claude {}: {error}", command.args.join(" ")))?;
        let line = format!("claude {}", command.args.join(" "));
        if !output.status.success() {
            return Err(format!(
                "{line}: {}\n{}\n{}",
                output.status,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        if !command.args.iter().any(|arg| arg == "list") {
            self.ran.push(line);
        }
        Ok(output.stdout)
    }

    fn checkpoint(&mut self, receipt: &PluginReceipt) -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(receipt).map_err(|error| error.to_string())?;
        fastctx_platform::publish(&self.record, &bytes, Permissions::Private)
            .map_err(|error| format!("keeping {}: {error}", self.record.display()))
    }
}

fn read_plugin_record(path: &Path) -> Result<Option<PluginReceipt>, Error> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| Error::Io(format!("reading {}: {error}", path.display()))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(Error::Io(format!("reading {}: {error}", path.display()))),
    }
}

/// The old fastctx's record of what it applied, in its settings file.
fn read_legacy(root: &Path) -> Result<Option<legacy::Receipt>, Error> {
    let path = root.join("config.toml");
    match std::fs::read(&path) {
        Ok(bytes) => Ok(legacy::Receipt::from_bytes(&bytes)?),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(Error::Io(format!("reading {}: {error}", path.display()))),
    }
}

/// Adds the change that takes `file` from what it was read as to `after`
/// (`None`: no file), when that is a change.
fn change(changes: &mut Vec<FileChange>, file: Snapshot, after: Option<Vec<u8>>) {
    match after {
        Some(bytes) if file.bytes() != Some(bytes.as_slice()) => changes.push(FileChange::write(file, bytes)),
        None if file.exists() => changes.push(FileChange::delete(file)),
        _ => {}
    }
}

/// Whether the copy at `installed` is already the fastctx at `running`.
fn same_executable(running: &Path, installed: &Snapshot) -> Result<bool, Error> {
    let Some(placed) = installed.bytes() else { return Ok(false) };
    if std::fs::canonicalize(running).ok() == std::fs::canonicalize(installed.path()).ok() {
        return Ok(true);
    }
    let running_bytes =
        std::fs::read(running).map_err(|error| Error::Io(format!("reading {}: {error}", running.display())))?;
    Ok(running_bytes == placed)
}

/// Whether two paths name the same place, however each is spelled.
fn same_path(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// A path as the engines compare it with their receipts.
fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn io(what: &'static str) -> impl Fn(io::Error) -> Error {
    move |error| Error::Io(format!("{what}: {error}"))
}
