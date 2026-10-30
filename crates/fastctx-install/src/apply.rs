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
//! added to Claude is taken back. File identities and both sides of publication
//! are recorded durably before replacing names. A restart resumes publication
//! or rollback. Displaced objects remain retained, including subsequent writes
//! through old handles; conflicting versions are named and never discarded.
//!
//! An unapply takes out what the record says an apply put in, and leaves
//! what the user changed since, saying where. It leaves fastctx's own copy,
//! which the node's service also starts from.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

use fastctx_platform::FileLock;
use fastctx_types::layout::WorldDir;
use serde::{Deserialize, Serialize};

use crate::briefing;
use crate::claude::plugins::{self, PluginApply, PluginCommand, PluginFailure, PluginHost, PluginReceipt};
use crate::claude::settings::{self, Limit, SettingsReceipt};
use crate::codex::{self, Approval, Ownership};
use crate::plan::{ClaudePlan, CodexPlan};
use crate::transaction::{Created, FileChange, FileError, Snapshot, Transaction};
use crate::{File, Notice, executable, guidance, legacy};

mod homes;
mod identity;
pub mod kernel;
mod preview;
mod progress;
mod recovery;
mod refresh;
mod settle;
mod tools;
mod withdrawn;
pub use homes::Host;
pub use identity::forward_login_identity;
pub use preview::preview;
pub use progress::Progress;
use progress::tell;
pub use refresh::{Refresh, refresh};
pub use settle::settle;
pub use tools::ToolCheck;
pub use withdrawn::{Integration, integrations, read_withdrawn, remember_withdrawn, restore_withdrawn};

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

    /// Selects one host's directory without changing the other host or the
    /// world's paths. Relative paths are resolved before any CLI is started.
    pub fn at_home(&self, host: Host, home: &Path) -> Result<Self, Error> {
        let mut places = self.clone();
        let home = homes::resolve(home)?;
        match host {
            Host::Codex => places.codex_home = home,
            Host::Claude => places.claude_home = home,
        }
        Ok(places)
    }

    fn lock(&self) -> PathBuf {
        self.root.join("apply.lock")
    }

    fn journal(&self) -> PathBuf {
        self.root.join("applied-files-pending.json")
    }
    fn stamp(&self) -> PathBuf {
        self.root.join("applied-operation.json")
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

/// Whether the sessions a host opens here have the World's tools.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Standing {
    /// The host is not installed here.
    Absent,
    /// Installed, and nothing of the World's is in it.
    Out,
    /// The record says an apply put the World's tools in.
    In,
    /// An old fastctx (0.1.0 to 0.2.6) put its own tools in, which an
    /// apply takes over.
    Old,
}

/// Ownership from the record, together with current native tool checks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    pub codex: Standing,
    pub claude: Standing,
    /// Whether the host's own replaced tools are off, from `status_checked`.
    pub codex_tools: Option<Box<ToolCheck>>,
    pub claude_tools: Option<Box<ToolCheck>>,
    /// What has been done with what an old fastctx left here.
    pub legacy: Option<crate::upgrade::Summary>,
}

/// How each host stands, as the record says; reads nothing of the hosts'.
/// The tool checks are left out: `status_checked` makes them.
pub fn status(places: &Places) -> Result<Status, Error> {
    let records = homes::Records::read(&Snapshot::read(places.record())?, places)?;
    let record = records.selected(places, Hosts { codex: true, claude: true })?;
    let present = Hosts::present(places);
    let standing = |present: bool, applied: bool| match (present, applied) {
        (_, true) => Standing::In,
        (true, false) => Standing::Out,
        (false, false) => Standing::Absent,
    };
    let plugin = load_plugin_record(&homes::plugin_record(places)?)?.is_some_and(|plugin| plugin.receipt.has_work());
    // The old fastctx kept its record in its settings, only for Codex.
    let old = record.codex.is_none() && homes::legacy(places)?.is_some();
    Ok(Status {
        codex: if old {
            Standing::Old
        } else {
            standing(present.codex, record.codex.as_ref().is_some_and(codex::Receipt::has_work))
        },
        claude: standing(
            present.claude,
            record.claude_md.is_some()
                || record.settings.is_some()
                || record.description.is_some()
                || record.claude_tools.is_some()
                || record.consent.is_some()
                || plugin,
        ),
        codex_tools: None,
        claude_tools: None,
        legacy: crate::upgrade::summary(places)?,
    })
}

/// Reads the integration of this specific host directory, without running a
/// host process. The directory need not be the environment's default.
pub fn status_at(places: &Places, host: Host, home: &Path) -> Result<Standing, Error> {
    let places = places.at_home(host, home)?;
    let records = homes::Records::read(&Snapshot::read(places.record())?, &places)?;
    let record = records.selected(&places, host.only())?;
    Ok(match host {
        Host::Codex if record.codex.as_ref().is_some_and(codex::Receipt::has_work) => Standing::In,
        Host::Codex if homes::legacy(&places)?.is_some() => Standing::Old,
        Host::Codex if places.codex_home.is_dir() => Standing::Out,
        Host::Codex => Standing::Absent,
        Host::Claude
            if !record.holds_nothing()
                || load_plugin_record(&homes::plugin_record(&places)?)?
                    .is_some_and(|plugin| plugin.receipt.has_work()) =>
        {
            Standing::In
        }
        Host::Claude if places.claude.is_some() => Standing::Out,
        Host::Claude => Standing::Absent,
    })
}

/// `status`, with the host's own tools checked where the World's are in:
/// checking Codex runs it once, so only an interface showing them asks.
pub fn status_checked(places: &Places) -> Result<Status, Error> {
    let mut status = status(places)?;
    if status.codex == Standing::In {
        status.codex_tools = Some(Box::new(tools::codex(places)));
    }
    if status.claude == Standing::In {
        status.claude_tools = Some(Box::new(tools::claude(places)));
    }
    Ok(status)
}

/// Whether Claude Code here starts the World's sessions without approvals:
/// the user accepted its bypass-permissions dialog (`claude::consent`).
/// Where Claude Code has never kept settings, nothing was accepted.
pub fn claude_consent(places: &Places) -> Result<bool, Error> {
    if !places.claude_home.is_dir() {
        return Ok(false);
    }
    let settings = Snapshot::read(places.claude_home.join("settings.json"))?;
    crate::claude::consent::accepted(settings.bytes()).map_err(Error::Claude)
}

/// Reads bypass-permissions consent in the selected Claude directory.
pub fn claude_consent_at(places: &Places, home: &Path) -> Result<bool, Error> {
    claude_consent(&places.at_home(Host::Claude, home)?)
}

/// Records consent in the selected Claude directory under the install lock.
pub fn consent_claude_at(places: &Places, home: &Path, accept: bool) -> Result<bool, Error> {
    consent_claude(&places.at_home(Host::Claude, home)?, accept)
}

/// Records the user's word on it, given on one of the user's own devices,
/// and says whether the settings changed. Claude Code rewrites the file
/// itself, so the edit is published only if it is still as it was read.
pub fn consent_claude(places: &Places, accept: bool) -> Result<bool, Error> {
    crate::disk::ensure_private_dir(&places.root).map_err(io("making fastctx's directory"))?;
    let _lock = FileLock::acquire(&places.lock()).map_err(io("taking the apply lock"))?;
    let settings = Snapshot::read(places.claude_home.join("settings.json"))?;
    let Some(bytes) = crate::claude::consent::set(settings.bytes(), accept).map_err(Error::Claude)? else {
        return Ok(false);
    };
    Transaction::new([FileChange::write(settings, bytes)])
        .commit()
        .map_err(|failure| Error::Files(failure.to_string()))?;
    Ok(true)
}

/// What an apply or unapply did, for the person who asked.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Report {
    /// Facts about the resulting configuration and effects of synchronization.
    pub messages: Vec<String>,
    /// Files written or removed.
    pub changed: Vec<PathBuf>,
    /// The `claude` commands that changed Claude Code, as run.
    pub commands: Vec<String>,
    /// Places left alone, and why.
    pub notices: Vec<Notice>,
    /// What was done in full but left something behind, such as an old copy
    /// of fastctx still running.
    pub leftovers: Vec<String>,
    pub preserved: Vec<PathBuf>,
    /// Post-commit checks. A failed check does not undo a committed install.
    pub tools: Vec<ToolCheck>,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Io(String),
    /// A renewal of the World program held its gates: nothing was changed,
    /// and the same done again once it lets go goes on.
    #[error("placing the World program: {0}")]
    Replacing(String),
    #[error(transparent)]
    Edit(#[from] crate::Error),
    #[error("Claude Code: {0}")]
    Claude(String),
    #[error("Claude Code's plugin: {0}")]
    Plugin(#[from] PluginFailure),
    #[error("file transaction failed: {0}")]
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
    /// The limit on a tool's description, raised so the whole of `run`
    /// reaches the model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    description: Option<SettingsReceipt>,
    #[serde(default)]
    claude_tools: Option<crate::claude::tools::Receipt>,
    #[serde(default)]
    settings_created: bool,
    /// The permission for the World's Claude conversations, granted with
    /// the apply (`claude::consent`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    consent: Option<crate::claude::consent::Receipt>,
    /// Files and directories an apply created, which an unapply may remove
    /// when they are empty or unchanged.
    created: Created,
}

const RECORD_VERSION: u32 = 1;

impl Record {
    fn read(snapshot: &Snapshot) -> Result<Self, Error> {
        match snapshot.bytes() {
            None => Ok(Self { version: RECORD_VERSION, ..Self::default() }),
            Some(bytes) => {
                let record: Self = serde_json::from_slice(bytes)
                    .map_err(|error| Error::Io(format!("reading {}: {error}", snapshot.path().display())))?;
                if record.version != RECORD_VERSION {
                    return Err(Error::Io(format!(
                        "{}: unsupported apply record version {}",
                        snapshot.path().display(),
                        record.version
                    )));
                }
                Ok(record)
            }
        }
    }

    fn holds_nothing(&self) -> bool {
        self.codex.is_none()
            && self.claude_md.is_none()
            && self.settings.is_none()
            && self.description.is_none()
            && self.claude_tools.is_none()
            && self.consent.is_none()
    }
}

/// What placing the World program came to when it failed: a renewal holding
/// its gates is said apart from the rest.
fn placing(error: anyhow::Error) -> Error {
    if error.chain().any(|cause| cause.is::<crate::renewal::Busy>()) {
        Error::Replacing(format!("{error:#}"))
    } else {
        Error::Io(format!("placing the World program: {error:#}"))
    }
}

/// Places this fastctx where the hosts and the node's service start it
/// from, as an apply does; returns that path, and what was changed or left.
pub fn place_executable(places: &Places) -> Result<(PathBuf, Report), Error> {
    let installed = crate::placement::ensure(&places.root, &places.fastctx).map_err(placing)?;
    crate::disk::ensure_private_dir(&places.root).map_err(io("making fastctx's directory"))?;
    let _lock = FileLock::acquire(&places.lock()).map_err(io("taking the apply lock"))?;
    let mut report = recovery::before(places)?;
    report.messages.push(format!("World program: {}", installed.display()));
    Ok((installed, report))
}

/// What an apply does with what the World put in that the user changed since.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Edits {
    /// Left as it is: the apply stops there, saying where.
    #[default]
    Keep,
    /// Rewritten as this fastctx writes it, Codex's server entry and the
    /// guidance blocks; the rest of each file stays the user's.
    TakeBack,
}

/// Puts the World's tools into the sessions `hosts` open from now on.
/// `approval` says whether Codex's shared output limit may be raised when
/// the user set it lower than the World needs.
pub fn apply(places: &Places, hosts: Hosts, approval: Approval) -> Result<Report, Error> {
    apply_reporting(places, hosts, approval, Edits::Keep, None)
}

/// `apply`, doing `edits` with what the user changed of the World's, and
/// telling `progress` each step as it goes.
pub fn apply_reporting(
    places: &Places,
    hosts: Hosts,
    approval: Approval,
    edits: Edits,
    progress: Option<Progress>,
) -> Result<Report, Error> {
    if places.root.join("uninstalled.json").try_exists().map_err(io("checking uninstall state"))? {
        return Err(Error::Io("this World was uninstalled; reinstall it before reconnecting hosts".into()));
    }
    tell(&progress, progress::Step::Program, progress::StepState::Running);
    crate::placement::ensure(&places.root, &places.fastctx).map_err(placing)?;
    tell(&progress, progress::Step::Program, progress::StepState::Done);
    if let Some(report) = progress::delegated(&progress, || kernel::delegate(places, hosts, approval, edits, true))? {
        return Ok(report);
    }
    crate::disk::ensure_private_dir(&places.root).map_err(io("making fastctx's directory"))?;
    let _lock = FileLock::acquire(&places.lock()).map_err(io("taking the apply lock"))?;
    if places.root.join("uninstalled.json").try_exists().map_err(io("checking uninstall state"))? {
        return Err(Error::Io("this World is being uninstalled".into()));
    }
    let mut report = recovery::before(places)?;
    let operation = recovery::id()?;
    let record_file = Snapshot::read(places.record())?;
    let mut records = homes::Records::read(&record_file, places)?;
    let mut record = records.selected(places, hosts)?;
    let installed = places.installed();
    let mut changes = Vec::new();

    if hosts.codex {
        let config = Snapshot::read(places.codex_home.join("config.toml"))?;
        let agents = Snapshot::read(places.codex_home.join("AGENTS.md"))?;
        // The old fastctx (0.1.0 to 0.2.6) kept its record in its settings.
        let legacy = if record.codex.is_none() { homes::legacy(places)? } else { None };
        let (old_file, mut old) = legacy.map(|(file, receipt)| (Some(file), Some(receipt))).unwrap_or_default();
        if let Some(old) = &mut old {
            bind_legacy(old, &config, &agents)?;
        }
        let ownership = match (&record.codex, &old) {
            (Some(receipt), _) if edits == Edits::TakeBack => Ownership::TakeBack(receipt),
            (Some(receipt), _) => Ownership::Current(receipt),
            (None, Some(receipt)) => Ownership::Legacy(receipt),
            (None, None) => Ownership::None,
        };
        let (config_path, agents_path) = (text(config.path()), text(agents.path()));
        let applied = codex::apply(
            File { path: &config_path, bytes: config.bytes() },
            File { path: &agents_path, bytes: agents.bytes() },
            &CodexPlan::new(&installed),
            &briefing::guidance(true),
            ownership,
            approval,
        )?;
        change(&mut changes, config, Some(applied.config));
        change(&mut changes, agents, Some(applied.agents));
        record.codex = Some(applied.receipt);
        if record.codex.as_ref().is_some_and(codex::Receipt::portable)
            && !codex::portable_on_path(&places.codex_home).map_err(Error::Io)?
        {
            report.messages.push(format!("{}: command = \"fastctx\" is preserved, but fastctx was not found on this process's PATH; Codex cannot start the World's tools unless fastctx is on the PATH Codex inherits", config_path));
        }

        report.notices.extend(applied.notices);
        report.messages.extend(applied.messages);
        if old.is_some() {
            retire_legacy(&mut changes, old_file.expect("legacy receipt came from this snapshot"))?;
        }
    }

    let mut plugin = None;
    if hosts.claude {
        let plugin_record = homes::plugin_record(places)?;
        let claude = places.claude.clone().ok_or(Error::NoClaude)?;
        // Nothing goes into a Claude Code the World does not work with.
        claude_supported(&claude, places)?;
        let initial_settings = Snapshot::read(places.claude_home.join("settings.json"))?;
        let claude_md = Snapshot::read(places.claude_home.join("CLAUDE.md"))?;
        let path = text(claude_md.path());
        let guide = if edits == Edits::TakeBack { guidance::take_back } else { guidance::apply };
        let applied = guide(
            File { path: &path, bytes: claude_md.bytes() },
            &briefing::guidance(false),
            record.claude_md.as_ref(),
        )?;
        let previous = read_plugin_record(&plugin_record, initial_settings.path(), &record)?;
        fastctx_hosts::claude::write_plugin(&places.world).map_err(io("writing the World's plugin"))?;
        let plan = ClaudePlan::new(&places.world);
        settings::apply(initial_settings.bytes(), &plan, record.settings.as_ref(), edits == Edits::TakeBack)
            .map_err(Error::Claude)?;
        settings::apply_limit(
            initial_settings.bytes(),
            Limit::description(&plan),
            record.description.as_ref(),
            edits == Edits::TakeBack,
        )
        .map_err(Error::Claude)?;
        let settings_path = text(initial_settings.path());
        crate::claude::tools::apply(
            File { path: &settings_path, bytes: initial_settings.bytes() },
            record.claude_tools.as_ref(),
            edits == Edits::TakeBack,
        )
        .map_err(Error::Claude)?;
        let settings_created = record.settings_created
            || previous.as_ref().is_some_and(|r| r.settings_created)
            || !initial_settings.exists();
        let mut host = Cli::new(claude, plugin_record, initial_settings.path().to_owned(), settings_created);
        host.progress = progress.clone();
        let before = plugins::snapshot(&mut host).map_err(Error::Claude)?;
        let planned =
            PluginApply::prepare(&plan, before, previous.as_ref().map(|r| &r.receipt)).map_err(Error::Claude)?;
        host.operation =
            Some(recovery::PendingPlugin { id: operation.clone(), plan: planned.clone(), withdrawing: false });
        let added = match planned.execute(&mut host) {
            Ok(receipt) => receipt,
            Err(failure) => {
                let mut message = failure.to_string();
                if failure.rollback_error.is_none()
                    && let Err(error) =
                        recovery::finish_rollback(places, &planned, &failure.receipt, &mut host, &mut report)
                {
                    message.push_str(&format!("\n{error}"));
                }
                return Err(Error::Claude(message));
            }
        };
        report.commands.append(&mut host.ran);

        // Read only now: the plugin commands write Claude's settings.
        let edit = (|| {
            let settings_file = Snapshot::read(places.claude_home.join("settings.json"))?;
            let edit =
                settings::apply(settings_file.bytes(), &plan, record.settings.as_ref(), edits == Edits::TakeBack)
                    .map_err(Error::Claude)?;
            settings_notice(&mut report, settings_file.path(), Limit::output(&plan).key, &edit);
            let described = settings::apply_limit(
                edit.bytes.as_deref(),
                Limit::description(&plan),
                record.description.as_ref(),
                edits == Edits::TakeBack,
            )
            .map_err(Error::Claude)?;
            settings_notice(&mut report, settings_file.path(), Limit::description(&plan).key, &described);
            let path = text(settings_file.path());
            let tools = crate::claude::tools::apply(
                File { path: &path, bytes: described.bytes.as_deref() },
                record.claude_tools.as_ref(),
                edits == Edits::TakeBack,
            )
            .map_err(Error::Claude)?;
            report.notices.extend(tools.notices);
            let (bytes, consent) = crate::claude::consent::grant(tools.bytes.as_deref(), record.consent.as_ref())
                .map_err(Error::Claude)?;
            change(&mut changes, settings_file, bytes);
            Ok::<_, Error>((edit, tools.receipt, consent, described.receipt))
        })();
        let edit = match edit {
            Ok(edit) => edit,
            Err(error) => return Err(rollback_plugin(places, error, &planned, &added, &mut host)),
        };
        record.settings = edit.0.receipt;
        record.claude_tools = Some(edit.1);
        record.consent = edit.2;
        record.description = edit.3;
        record.settings_created = settings_created;
        change(&mut changes, claude_md, Some(applied.bytes));
        record.claude_md = Some(applied.receipt);
        plugin = Some((planned, added, host));
    }

    record.version = RECORD_VERSION;
    // fastctx's own copy, and the directory it lives in, belong to fastctx,
    // not to what an unapply takes out.
    let bin = installed.parent().map(Path::to_path_buf);
    changes.push(recovery::stamp(places, &operation)?);
    tell(&progress, progress::Step::Files { changed: Vec::new() }, progress::StepState::Running);
    let committed = Transaction::new(changes).commit_with_record_durable(places.journal(), record_file, |created| {
        let mut record = record.clone();
        record.created.files.extend(created.files.iter().filter(|file| !same_path(file, &installed)).cloned());
        record.created.directories.extend(
            created
                .directories
                .iter()
                .filter(|directory| bin.as_deref().is_none_or(|bin| !same_path(&directory.path, bin)))
                .cloned(),
        );
        records.update(places, hosts, &record).map_err(io::Error::other)?;
        serde_json::to_vec_pretty(&records).map_err(io::Error::other)
    });
    match committed {
        Ok(receipt) => {
            tell(&progress, progress::Step::Files { changed: receipt.changed.clone() }, progress::StepState::Done);
            report.changed.extend(receipt.changed);
            report.preserved.extend(receipt.preserved);
            report.leftovers.extend(receipt.cleanup.iter().map(ToString::to_string));
            report.leftovers.extend(executable::cleanup_retired(&installed).iter().map(ToString::to_string));
            if let Some((_, added, mut host)) = plugin {
                host.operation = None;
                host.checkpoint(&added).map_err(Error::Claude)?;
            }
            if hosts.codex {
                report.tools.push(progress::checked(&progress, || tools::codex(places)));
            }
            if hosts.claude {
                report.tools.push(progress::checked(&progress, || tools::claude(places)));
            }
            Ok(report)
        }
        Err(failure) => {
            // File rollback failures are already in the error. Withdraw this
            // invocation's plugin additions even when a file could not be restored.
            let error = Error::Files(failure.to_string());
            if let Some((planned, added, mut host)) = plugin {
                return Err(rollback_plugin(places, error, &planned, &added, &mut host));
            }
            Err(error)
        }
    }
}

/// Takes out of `hosts` what an apply put in, leaving what the user changed
/// since.
pub fn unapply(places: &Places, hosts: Hosts) -> Result<Report, Error> {
    unapply_reporting(places, hosts, None)
}

/// `unapply`, telling `progress` each step as it goes.
pub fn unapply_reporting(places: &Places, hosts: Hosts, progress: Option<Progress>) -> Result<Report, Error> {
    if let Some(report) =
        progress::delegated(&progress, || kernel::delegate(places, hosts, Approval::Require, Edits::Keep, false))?
    {
        return Ok(report);
    }
    crate::disk::ensure_private_dir(&places.root).map_err(io("making fastctx's directory"))?;
    let _lock = FileLock::acquire(&places.lock()).map_err(io("taking the apply lock"))?;
    let mut report = recovery::before(places)?;
    let operation = recovery::id()?;
    let record_file = Snapshot::read(places.record())?;
    let mut records = homes::Records::read(&record_file, places)?;
    let mut record = records.selected(places, hosts)?;
    let mut changes = Vec::new();

    if hosts.codex {
        let legacy = if record.codex.is_none() { homes::legacy(places)? } else { None };
        let (old_file, mut old) = legacy.map(|(file, receipt)| (Some(file), Some(receipt))).unwrap_or_default();
        if record.codex.is_some() || old.is_some() {
            let config = Snapshot::read(places.codex_home.join("config.toml"))?;
            let agents = Snapshot::read(places.codex_home.join("AGENTS.md"))?;
            if let Some(old) = &mut old {
                bind_legacy(old, &config, &agents)?;
            }
            let (config_path, agents_path) = (text(config.path()), text(agents.path()));
            let config_input = File { path: &config_path, bytes: config.bytes() };
            let agents_input = File { path: &agents_path, bytes: agents.bytes() };
            let taken = if let Some(old) = &old {
                codex::unapply_legacy(config_input, agents_input, old)?
            } else {
                codex::unapply(config_input, agents_input, record.codex.as_ref())?
            };
            change(&mut changes, config, taken.config);
            change(&mut changes, agents, taken.agents);
            record.codex = taken.receipt;
            report.notices.extend(taken.retained);
            report.messages.extend(taken.messages);
            if old.is_some() {
                retire_legacy(&mut changes, old_file.expect("legacy receipt came from this snapshot"))?;
            }
        }
    }

    let mut plugin_done = false;
    let mut plugin = None;
    let plugin_record = if hosts.claude { Some(homes::plugin_record(places)?) } else { None };
    if hosts.claude {
        let plugin_record = plugin_record.as_ref().expect("selected Claude record");
        let initial_settings = Snapshot::read(places.claude_home.join("settings.json"))?;
        // Validate the profile and guidance before any irreversible CLI command.
        let guidance = if let Some(receipt) = record.claude_md.as_ref() {
            let file = Snapshot::read(places.claude_home.join("CLAUDE.md"))?;
            let path = text(file.path());
            let taken = guidance::unapply(File { path: &path, bytes: file.bytes() }, Some(receipt))?;
            Some((file, taken))
        } else {
            None
        };
        let previous = read_plugin_record(plugin_record, initial_settings.path(), &record)?;
        if let Some(claude) = places.claude.clone() {
            record.settings_created |= previous.as_ref().is_some_and(|record| record.settings_created);
            let mut host =
                Cli::new(claude, plugin_record.clone(), initial_settings.path().to_owned(), record.settings_created);
            host.progress = progress.clone();
            let plan = ClaudePlan::new(&places.world);
            let baseline = plugins::snapshot(&mut host).map_err(Error::Claude)?;
            let baseline = PluginApply::prepare(&plan, baseline, previous.as_ref().map(|record| &record.receipt))
                .map_err(Error::Claude)?;
            host.operation = Some(recovery::PendingPlugin { id: operation.clone(), plan: baseline, withdrawing: true });
            let remaining =
                plugins::unapply_managed(&mut host, &plan, previous.as_ref().map(|record| &record.receipt))?;
            report.commands.append(&mut host.ran);
            plugin_done = !remaining.has_work();
            plugin = Some((remaining, host));
        } else if previous.as_ref().is_some_and(|record| record.receipt.has_work()) {
            return Err(Error::NoClaude);
        }
        let settings_file = Snapshot::read(places.claude_home.join("settings.json"))?;
        let mut settings_bytes =
            crate::claude::registration::remove(settings_file.bytes(), &ClaudePlan::new(&places.world), true, true)
                .map_err(Error::Claude)?;
        if let Some(receipt) = record.claude_tools.take() {
            let path = text(settings_file.path());
            let edit = crate::claude::tools::unapply(File { path: &path, bytes: settings_bytes.as_deref() }, &receipt)
                .map_err(Error::Claude)?;
            report.notices.extend(edit.notices);
            settings_bytes = edit.bytes;
        }
        if let Some(receipt) = record.settings.take() {
            let edit = settings::unapply(settings_bytes.as_deref(), Some(&receipt)).map_err(Error::Claude)?;
            settings_notice(&mut report, settings_file.path(), "MAX_MCP_OUTPUT_TOKENS", &edit);
            settings_bytes = edit.bytes;
            record.settings = edit.receipt;
        }
        if let Some(receipt) = record.description.take() {
            let key = Limit::description(&ClaudePlan::new(&places.world)).key;
            let edit =
                settings::unapply_limit(settings_bytes.as_deref(), key, Some(&receipt)).map_err(Error::Claude)?;
            settings_notice(&mut report, settings_file.path(), key, &edit);
            settings_bytes = edit.bytes;
            record.description = edit.receipt;
        }
        if let Some(receipt) = record.consent.take() {
            let (bytes, kept) =
                crate::claude::consent::restore(settings_bytes.as_deref(), &receipt).map_err(Error::Claude)?;
            if kept {
                report.notices.push(consent_kept(settings_file.path()));
            }
            settings_bytes = bytes;
        }
        if record.settings_created {
            settings_bytes = settings::discard_empty_created(settings_bytes).map_err(Error::Claude)?;
        }
        change(&mut changes, settings_file, settings_bytes);
        record.settings_created = false;
        if let Some((claude_md, taken)) = guidance {
            change(&mut changes, claude_md, taken.bytes);
            record.claude_md = taken.receipt;
            report.notices.extend(taken.retained);
        }
    }

    changes.push(recovery::stamp(places, &operation)?);
    let cleanup = records.update(places, hosts, &record)?;
    tell(&progress, progress::Step::Files { changed: Vec::new() }, progress::StepState::Running);
    let committed = if records.is_empty() {
        if record_file.exists() {
            changes.push(FileChange::delete(record_file));
        }
        Transaction::new(changes).cleanup_after_commit(&cleanup).commit_durable(places.journal())
    } else {
        Transaction::new(changes).cleanup_after_commit(&cleanup).commit_with_record_durable(
            places.journal(),
            record_file,
            |_| serde_json::to_vec_pretty(&records).map_err(io::Error::other),
        )
    };
    let receipt = committed.map_err(|failure| Error::Files(failure.to_string()))?;
    tell(&progress, progress::Step::Files { changed: receipt.changed.clone() }, progress::StepState::Done);
    report.changed.extend(receipt.changed);
    report.preserved.extend(receipt.preserved);
    report.leftovers.extend(receipt.cleanup.iter().map(ToString::to_string));
    if let Some((remaining, mut host)) = plugin {
        host.operation = None;
        host.settings_created = false;
        host.checkpoint(&remaining).map_err(Error::Claude)?;
    }
    if plugin_done {
        let plugin_record = plugin_record.expect("Claude was withdrawn");
        match std::fs::remove_file(&plugin_record) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => report.leftovers.push(format!("{}: {error}", plugin_record.display())),
        }
    }
    Ok(report)
}

/// Runs the public `claude` command for the plugin engine, and keeps each
/// checkpoint it hands over before the next command runs.
struct Cli {
    claude: PathBuf,
    record: PathBuf,
    settings_path: PathBuf,
    settings_created: bool,
    operation: Option<recovery::PendingPlugin>,
    /// The commands that changed Claude, as run.
    ran: Vec<String>,
    /// Told each command that changes Claude, as it runs.
    progress: Option<Progress>,
}

impl Cli {
    fn new(claude: PathBuf, record: PathBuf, settings_path: PathBuf, settings_created: bool) -> Self {
        Self { claude, record, settings_path, settings_created, operation: None, ran: Vec::new(), progress: None }
    }
}

impl PluginHost for Cli {
    fn run(&mut self, command: &PluginCommand) -> Result<Vec<u8>, String> {
        let mutation = !command.args.iter().any(|arg| arg == "list");
        let root = self.record.parent().expect("record directory");
        if mutation {
            // The settings as they were before the command are held only by
            // a file transaction this command compensates for, and go with it.
            let before = Snapshot::read(&self.settings_path).map_err(|error| error.to_string())?;
            crate::transaction::prepare_settings_command(&root.join("applied-files-pending.json"), &before)
                .map_err(|error| error.to_string())?;
        }
        let line = format!("claude {}", command.args.join(" "));
        let step = progress::Step::Command { command: line.clone() };
        if mutation {
            tell(&self.progress, step.clone(), progress::StepState::Running);
        }
        let ran = (|| {
            let program = std::path::absolute(&self.claude).map_err(|e| e.to_string())?;
            let cwd =
                std::path::absolute(self.record.parent().expect("record directory")).map_err(|e| e.to_string())?;
            crate::claude::process::output(
                &program,
                &command.args,
                &cwd,
                self.settings_path.parent().expect("settings directory"),
            )
            .map_err(|error| format!("running claude {}: {error}", command.args.join(" ")))
        })();
        let output = match ran {
            Ok(output) => output,
            Err(error) => {
                if mutation {
                    tell(&self.progress, step, progress::StepState::Failed { reason: error.clone() });
                }
                return Err(error);
            }
        };
        if mutation {
            let state = if output.status.success() {
                progress::StepState::Done
            } else {
                progress::StepState::Failed { reason: format!("{line}: {}", output.status) }
            };
            tell(&self.progress, step, state);
        }
        if !output.status.success() {
            return Err(format!(
                "{line}: {}\n{}\n{}",
                output.status,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            ));
        }
        if mutation {
            self.ran.push(line);
        }
        Ok(output.stdout)
    }

    fn checkpoint(&mut self, receipt: &PluginReceipt) -> Result<(), String> {
        let record = PluginRecord {
            settings_path: Some(self.settings_path.clone()),
            settings_created: self.settings_created,
            receipt: receipt.clone(),
            operation: self.operation.clone(),
        };
        write_plugin_record(&self.record, &record)
    }
}

#[derive(Serialize, Deserialize)]
struct PluginRecord {
    #[serde(default)]
    settings_path: Option<PathBuf>,
    #[serde(default)]
    settings_created: bool,
    #[serde(default)]
    operation: Option<recovery::PendingPlugin>,
    #[serde(flatten)]
    receipt: PluginReceipt,
}

fn read_plugin_record(path: &Path, settings: &Path, record: &Record) -> Result<Option<PluginRecord>, Error> {
    let Some(plugin) = load_plugin_record(path)? else { return Ok(None) };
    let previous = plugin
        .settings_path
        .clone()
        .or_else(|| record.claude_md.as_ref().map(|receipt| Path::new(&receipt.path).with_file_name("settings.json")));
    if let Some(previous) = previous {
        homes::check_file(&previous, settings)?;
    }
    Ok(Some(plugin))
}

fn load_plugin_record(path: &Path) -> Result<Option<PluginRecord>, Error> {
    match std::fs::read(path) {
        Ok(bytes) => {
            let plugin: PluginRecord = serde_json::from_slice(&bytes)
                .map_err(|error| Error::Io(format!("reading {}: {error}", path.display())))?;
            Ok(Some(plugin))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(Error::Io(format!("reading {}: {error}", path.display()))),
    }
}

fn write_plugin_record(path: &Path, record: &PluginRecord) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(record).map_err(|error| error.to_string())?;
    crate::disk::publish_private(path, &bytes).map_err(|error| format!("keeping {}: {error}", path.display()))
}

fn retire_legacy(changes: &mut Vec<FileChange>, file: Snapshot) -> Result<(), Error> {
    let path = text(file.path());
    let retired = legacy::retire(File { path: &path, bytes: file.bytes() })?;
    change(changes, file, Some(retired));
    Ok(())
}

fn bind_legacy(receipt: &mut legacy::Receipt, config: &Snapshot, agents: &Snapshot) -> Result<(), Error> {
    // Released installers stored ordinary paths; Snapshot uses canonical
    // parents (including Windows' verbatim prefix). Resolve both before the
    // pure planners compare their identity strings.
    for (recorded, actual) in [(receipt.config_path(), config), (receipt.agents_path(), agents)] {
        let (original, _) = crate::snapshot::resolve(Path::new(recorded))
            .map_err(|error| FileError::io(Path::new(recorded), "resolve legacy path", error))?;
        if !same_path(&original, actual.path()) {
            return Err(crate::Error::at(text(actual.path()), format!("the receipt belongs to {recorded}")).into());
        }
    }
    receipt.bind_paths(text(config.path()), text(agents.path()));
    Ok(())
}

/// The permission's key, changed by the user since the apply granted it,
/// stays as they left it.
fn consent_kept(path: &Path) -> Notice {
    Notice {
        location: format!("{}: skipDangerousModePermissionPrompt", path.display()),
        reason: "changed since apply; left untouched".into(),
    }
}

fn settings_notice(report: &mut Report, path: &Path, key: &str, edit: &settings::SettingsEdit) {
    if edit.outcome == settings::SettingsOutcome::UserChanged {
        report.notices.push(Notice {
            location: format!("{}: env.{key}", path.display()),
            reason: "changed since apply; left untouched".into(),
        });
    }
}

fn rollback_plugin(
    places: &Places,
    error: Error,
    planned: &PluginApply,
    added: &PluginReceipt,
    host: &mut Cli,
) -> Error {
    match planned.rollback(host, added) {
        Ok(receipt) => match recovery::finish_rollback(places, planned, &receipt, host, &mut Report::default()) {
            Ok(()) => error,
            Err(rollback) => Error::Io(format!("{error}\n{rollback}")),
        },
        Err(rollback) => {
            Error::Io(format!("{error}\nand what was added to Claude Code could not be taken back: {rollback}"))
        }
    }
}

/// Adds the change that takes `file` from what it was read as to `after`
/// (`None`: no file). Unchanged inputs still guard receipt publication.
fn change(changes: &mut Vec<FileChange>, file: Snapshot, after: Option<Vec<u8>>) {
    match after {
        Some(bytes) => changes.push(FileChange::write(file, bytes)),
        None => changes.push(FileChange::delete(file)),
    }
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

/// Refuses the `claude` command `claude` when it is older than the World
/// works with, as it says of itself (`fastctx_hosts::claude::OLDEST`).
fn claude_supported(claude: &Path, places: &Places) -> Result<(), Error> {
    let program = std::path::absolute(claude).map_err(io("finding the claude command"))?;
    let said = crate::claude::process::output(&program, &["--version".to_string()], &places.root, &places.claude_home)
        .map_err(|error| Error::Claude(format!("running claude --version: {error}")))?;
    if !said.status.success() {
        return Err(Error::Claude(format!("claude --version exited {}", said.status)));
    }
    fastctx_hosts::claude::supported(String::from_utf8_lossy(&said.stdout).trim()).map_err(Error::Claude)
}

fn io(what: &'static str) -> impl Fn(io::Error) -> Error {
    move |error| Error::Io(format!("{what}: {error}"))
}
