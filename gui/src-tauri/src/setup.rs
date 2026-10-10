//! A computer meeting the World for the first time, and joining it: where
//! this computer stands (whether fastctx is in place, whether it is in a
//! World, whether a service keeps its node), putting the fastctx this
//! application carries in place, joining a World with a code, founding one
//! with a hub on this computer, and putting the World's tools into the
//! hosts here. Each step is the library the command line uses, run from
//! the copy of fastctx placed for the hosts, never from this program.

use std::path::PathBuf;

use fastctx_client::World;
use fastctx_install::apply::{self, Hosts, Places, Standing};
use fastctx_install::codex::Approval;
use fastctx_install::service::{Kept, Released, keep_connected, stop_keeping};
use fastctx_types::layout::WorldDir;
use serde::Serialize;

/// Where this computer stands.
#[derive(Serialize)]
pub struct Setup {
    /// The copy of fastctx the hosts and the service run, and whether it is
    /// there.
    pub installed: Option<String>,
    /// The fastctx this application carries, when this build carries one.
    pub carried: Option<String>,
    /// Whether the two are the same program.
    pub same: bool,
    /// Whether a system service keeps this World's node.
    pub service: bool,
    pub codex: &'static str,
    pub claude: &'static str,
    /// What was done with what an old fastctx left here; none when nothing
    /// old was found.
    pub legacy: Option<Legacy>,
}

/// The cleanup of an old fastctx, as its record says.
#[derive(Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Legacy {
    Done {
        /// The record of every step, `upgrade-clean.json`.
        record: String,
        /// The `legacy-archive-<day>` directories things were put in.
        archives: Vec<String>,
        archived: usize,
        tidied: usize,
        waiting: usize,
    },
    /// The record is there but could not be read.
    Unreadable { why: String },
}

/// What came of keeping this machine connected, in words for the page.
#[derive(Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum KeptNow {
    Already,
    Installed,
    Declined,
    Unsupported,
    /// The rights are asked for with this command, run by the user.
    Command {
        command: String,
    },
    /// The service is removed; the node runs while something here starts it.
    Removed,
    /// No service kept the node, so there was none to remove.
    NotKept,
}

fn standing(standing: Standing) -> &'static str {
    match standing {
        Standing::Absent => "absent",
        Standing::Out => "out",
        Standing::In => "in",
        Standing::Old => "old",
    }
}

/// The fastctx this application carries: beside its own program, where
/// the installer puts it.
pub(super) fn carried() -> Option<PathBuf> {
    let program = std::env::current_exe().ok()?;
    let beside = program.parent()?.join(if cfg!(windows) { "fastctx.exe" } else { "fastctx" });
    beside.is_file().then_some(beside)
}

/// This user's places, with the program to place being the one this
/// application carries.
pub(crate) fn places() -> Result<Places, String> {
    let mut places = Places::here().map_err(|error| error.to_string())?;
    if let Some(carried) = carried() {
        places.fastctx = carried;
    }
    Ok(places)
}

pub fn setup() -> Result<Setup, String> {
    let places = places()?;
    let installed = places.installed();
    let carried = carried();
    let same = match (&carried, installed.is_file()) {
        (Some(carried), true) => std::fs::read(carried).ok() == std::fs::read(&installed).ok(),
        _ => false,
    };
    let status = apply::status(&places).map_err(|error| error.to_string())?;
    let legacy = match fastctx_install::upgrade::summary(&places) {
        Ok(summary) => summary.map(|summary| Legacy::Done {
            record: summary.record.display().to_string(),
            archives: summary.archives.iter().map(|path| path.display().to_string()).collect(),
            archived: summary.archived,
            tidied: summary.tidied,
            waiting: summary.waiting,
        }),
        Err(error) => Some(Legacy::Unreadable { why: error.to_string() }),
    };
    Ok(Setup {
        installed: installed.is_file().then(|| installed.display().to_string()),
        carried: carried.map(|path| path.display().to_string()),
        same,
        service: WorldDir::under(&places.root).node_service().exists(),
        codex: standing(status.codex),
        claude: standing(status.claude),
        legacy,
    })
}

/// Puts the fastctx this application carries where the hosts and the
/// service run it from, and that directory on the user's PATH, so a
/// terminal opened afterwards has the command.
pub fn place() -> Result<String, String> {
    let places = places()?;
    if carried().is_none() {
        return Err("this build of World carries no fastctx to put in place".into());
    }
    let mut command = std::process::Command::new(&places.fastctx);
    command.arg("renew").arg("stage").arg("--dir").arg(&places.root).arg("--source").arg(&places.fastctx);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        command.creation_flags(0x08000000);
    }
    let result = command.output().map_err(|error| error.to_string())?;
    if !result.status.success() {
        return Err(String::from_utf8_lossy(&result.stderr).into_owned());
    }
    let installed = places.installed();
    let bin = installed.parent().ok_or("the placed fastctx has no directory")?;
    crate::user_path::add(bin)
        .map_err(|error| format!("{} is in place, but putting it on the PATH failed: {error}", installed.display()))?;
    Ok(installed.display().to_string())
}

/// Has a system service keep this machine's node, asking once for
/// administrator rights, and the node that served until now hand the World
/// over to it.
pub async fn keep(world: &World) -> Result<KeptNow, String> {
    let places = places()?;
    let (kept, _) = tauri::async_runtime::spawn_blocking(move || keep_connected(&places))
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?;
    if kept == Kept::Installed {
        world.hand_over().await.map_err(|error| error.to_string())?;
    }
    Ok(match kept {
        Kept::Already => KeptNow::Already,
        Kept::Installed => KeptNow::Installed,
        Kept::Declined => KeptNow::Declined,
        Kept::Unsupported => KeptNow::Unsupported,
        Kept::Command(command) => KeptNow::Command { command },
    })
}

/// Removes the service that keeps this machine's node, with the same
/// rights installing it took.
pub fn release() -> Result<KeptNow, String> {
    Ok(match stop_keeping(&places()?).map_err(|error| error.to_string())? {
        Released::NotKept => KeptNow::NotKept,
        Released::Removed => KeptNow::Removed,
        Released::Declined => KeptNow::Declined,
        Released::Command(command) => KeptNow::Command { command },
    })
}

/// Puts the World's tools into this computer's Codex and Claude Code, or
/// takes them out. `raise` lets Codex's shared output limit be raised.
pub fn hosts(codex: bool, claude: bool, raise: bool, into: bool, home: Option<PathBuf>) -> Result<Vec<String>, String> {
    let mut places = places()?;
    if let Some(home) = home {
        let host = match (codex, claude) {
            (true, false) => apply::Host::Codex,
            (false, true) => apply::Host::Claude,
            _ => return Err("a home directory requires exactly one host".into()),
        };
        places = places.at_home(host, &home).map_err(|error| error.to_string())?;
    }
    let hosts = Hosts { codex, claude };
    let report = if into {
        apply::apply(&places, hosts, if raise { Approval::RaiseOutputLimit } else { Approval::Require })
    } else {
        apply::unapply(&places, hosts)
    }
    .map_err(|error| error.to_string())?;
    Ok(report
        .notices
        .iter()
        .map(|notice| format!("{}: {}", notice.location, notice.reason))
        .chain(report.messages)
        .chain(report.leftovers)
        .collect())
}
