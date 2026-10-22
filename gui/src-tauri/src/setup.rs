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
use fastctx_install::service::{Kept, Released, keep_connected, stop_keeping};
use fastctx_types::layout::WorldDir;
use serde::Serialize;

mod hosts;
#[cfg(windows)]
mod membership;

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
    // A development build's fastctx stays off the PATH, which the installed
    // one's is on.
    if fastctx_platform::RELEASED {
        crate::user_path::add(bin).map_err(|error| {
            format!("{} is in place, but putting it on the PATH failed: {error}", installed.display())
        })?;
    }
    Ok(installed.display().to_string())
}

/// How soon the carried program is offered again while a renewal busy with
/// a step of its own refuses it, and how far apart at most once it has for
/// long: as long as the interface is open.
#[cfg(desktop)]
const TAKE_AGAIN: std::time::Duration = std::time::Duration::from_secs(30);
#[cfg(desktop)]
const TAKE_AGAIN_AT_MOST: std::time::Duration = std::time::Duration::from_secs(300);

/// However this application was updated, the World program it carries
/// replaces an older one in place by itself, nobody asked
/// (`world-upgrade.md`, 换版不要人确认): once each time it opens, in the
/// background. A failed renewal of that program is taken up again too.
#[cfg(desktop)]
pub fn take_carried() {
    static TAKING: std::sync::Once = std::sync::Once::new();
    TAKING.call_once(|| {
        std::thread::spawn(|| {
            let mut again = TAKE_AGAIN;
            loop {
                match carried_newer() {
                    Ok(false) => return,
                    Ok(true) => match place() {
                        Ok(_) => return,
                        Err(error) => eprintln!("World: putting the World program it carries in place: {error}"),
                    },
                    Err(error) => {
                        eprintln!("World: comparing the World program it carries with the one in place: {error}")
                    }
                }
                std::thread::sleep(again);
                again = (again * 2).min(TAKE_AGAIN_AT_MOST);
            }
        });
    });
}

/// Whether the program this application carries is a later release than
/// the World program in place, or the one a renewal left failed; never
/// before a first placement, which opening the interface makes.
#[cfg(desktop)]
fn carried_newer() -> Result<bool, String> {
    let places = places()?;
    let installed = places.installed();
    if carried().is_none() || !installed.is_file() {
        return Ok(false);
    }
    let offered = fastctx_install::program::inspect(&places.fastctx).map_err(|error| format!("{error:#}"))?;
    let failed = fastctx_install::renewal::read(&places.root)
        .map_err(|error| error.to_string())?
        .is_some_and(|plan| plan.phase == fastctx_types::renewal::Phase::Failed && plan.candidate.build == offered);
    let current = fastctx_install::program::inspect(&installed).map_err(|error| format!("{error:#}"))?;
    Ok(failed
        || (offered != current
            && fastctx_install::program::newer(&offered, &current).map_err(|error| format!("{error:#}"))?))
}

/// First placement finishes before the interface makes its first World
/// calls. The CLI stages an asynchronous handover even on a fresh root.
/// Where nothing put fastctx in place yet, opening the interface does: a
/// node is started from it, here as on a computer in no network.
#[cfg(desktop)]
pub fn place_initial() -> Result<(), String> {
    use fastctx_types::renewal::Phase;
    place()?;
    let root = places()?.root;
    let began = std::time::Instant::now();
    loop {
        let plan = fastctx_install::renewal::read(&root)
            .map_err(|error| error.to_string())?
            .ok_or("the initial executable placement has no renewal record")?;
        if began.elapsed() >= std::time::Duration::from_secs(60) {
            return Err(format!(
                "initial placement {} still at {:?} after 60 seconds: {}",
                fastctx_install::renewal::directory(&root).join("plan.json").display(),
                plan.phase,
                plan.error.as_deref().unwrap_or("no error reported")
            ));
        }
        match plan.phase {
            Phase::Complete => return Ok(()),
            Phase::Failed => return Err(plan.error.unwrap_or("the initial executable placement failed".into())),
            Phase::AwaitingLegacyExit => {
                return Err("the initial executable placement is waiting for an old node to exit".into());
            }
            // Published and serving, its boot service still to take it,
            // which goes on by itself.
            Phase::AwaitingAdministrator | Phase::UpdatingGuardian | Phase::ReturningToService
                if fastctx_install::renewal::digest(&fastctx_install::renewal::installed(&root))
                    .is_ok_and(|digest| digest == plan.candidate.build.digest) =>
            {
                return Ok(());
            }
            _ => std::thread::sleep(std::time::Duration::from_millis(20)),
        }
    }
}

/// Has a system service keep this machine's node, asking once for
/// administrator rights, and the node that served until now hand the World
/// over to it.
pub async fn keep(world: &World) -> Result<KeptNow, String> {
    let places = places()?;
    let (kept, _) = tauri::async_runtime::spawn_blocking(move || {
        let (kept, report) = keep_connected(&places).map_err(|error| error.to_string())?;
        #[cfg(target_os = "linux")]
        let kept = if matches!(kept, Kept::Command(_)) {
            crate::linux_service::run(
                fastctx_install::service::Kind::Node,
                "install",
                &places.root,
                &places.installed(),
            )?
        } else {
            kept
        };
        Ok::<_, String>((kept, report))
    })
    .await
    .map_err(|error| error.to_string())?
    .map_err(|error| error.to_string())?;
    finish_keep(world, kept).await
}

/// Completes the shared service handover after installation, including a
/// node installed together with the hub during local network creation. A
/// service already there may have a successor waiting too, for a node this
/// interface started before it was installed.
pub(crate) async fn finish_keep(world: &World, kept: Kept) -> Result<KeptNow, String> {
    // A service already there hands over only to a successor that waits.
    let waits = world.dir().node_successor().try_exists().unwrap_or(false);
    if kept == Kept::Installed || (kept == Kept::Already && waits) {
        yield_running(world.dir()).await?;
    }
    if kept == Kept::Installed {
        #[cfg(target_os = "linux")]
        tauri::async_runtime::spawn_blocking(|| {
            let places = crate::setup::places()?;
            crate::linux_service::wait_for_node(&places.root, &places.installed())
        })
        .await
        .map_err(|error| error.to_string())??;
    }
    Ok(match kept {
        Kept::Already => KeptNow::Already,
        Kept::Installed => KeptNow::Installed,
        Kept::Declined => KeptNow::Declined,
        Kept::Unsupported => KeptNow::Unsupported,
        Kept::Command(command) => KeptNow::Command { command },
    })
}

/// What installing World does for this computer besides the application:
/// the fastctx it carries put in place when there is none yet, the records
/// of a removal that kept the data cleared, and the host hook-ups that
/// removal took out put back (`world-gui.md`, 设置). An update finds the
/// program there and leaves it: a newer World is put in place by the user,
/// from the settings. A computer already in a network also gets back the
/// service that keeps it there from boot; on one in no network the service
/// comes when it first joins or creates one (`keep`), and until then a node
/// starts when an interface or an agent needs it.
#[cfg(windows)]
pub fn after_install() -> Result<(), String> {
    let places = places()?;
    let _uninstall = fastctx_platform::uninstall_gate::Gate::try_acquire(&places.root)
        .map_err(|error| error.to_string())?
        .ok_or("this World is still being uninstalled")?;
    fastctx_install::uninstall::delete_file(&places.root.join("uninstalled.json"))
        .map_err(|error| error.to_string())?;
    fastctx_install::uninstall::delete_file(&places.root.join("uninstall-progress.json"))
        .map_err(|error| error.to_string())?;
    // An installer run over a World program in place already is an update:
    // it changes nothing about how the machine is kept, and asks nobody for
    // anything (`world-upgrade.md`, 换版不要人确认).
    let first = !places.installed().is_file();
    if first {
        place()?;
    }
    apply::restore_withdrawn(&places).map_err(|error| error.to_string())?;
    if !first || !membership::any(&WorldDir::under(&places.root)).map_err(|error| error.to_string())? {
        return Ok(());
    }
    let (kept, _) = keep_connected(&places).map_err(|error| error.to_string())?;
    if kept == Kept::Installed {
        hand_over_running(&places)?;
    }
    Ok(())
}

/// Asks the node running for this World, if one is, to hand the World over
/// to the service's; none is started for the asking.
#[cfg(windows)]
fn hand_over_running(places: &Places) -> Result<(), String> {
    let world = WorldDir::under(&places.root);
    tauri::async_runtime::block_on(yield_running(&world))
}

/// Asks the node serving now, one no service keeps, to hand the World to
/// the service's; asked again once if the service's has not taken it by
/// then, as a request that never reached it left both waiting for good.
async fn yield_running(world: &WorldDir) -> Result<(), String> {
    for _ in 0..2 {
        match fastctx_client::renewal::probe(world).await.map_err(|error| error.to_string())? {
            fastctx_client::renewal::Probe::Absent => return Ok(()),
            fastctx_client::renewal::Probe::Running(node) if node.supervised => return Ok(()),
            fastctx_client::renewal::Probe::Running(node) => {
                if !node.yielding {
                    fastctx_client::renewal::request(world, fastctx_types::renewal::Action::Yield)
                        .await
                        .map_err(|error| error.to_string())?;
                }
            }
            fastctx_client::renewal::Probe::Legacy { .. } => {
                return Err("the legacy node must exit before the service can take over".into());
            }
        }
        if taken(world).await {
            return Ok(());
        }
    }
    Ok(())
}

/// Whether the service's node takes the World within a while: no successor
/// waits for it any more.
async fn taken(world: &WorldDir) -> bool {
    let waiting = world.node_successor();
    for _ in 0..80 {
        if !waiting.try_exists().unwrap_or(true) {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
    false
}

/// Removes the service that keeps this machine's node, with the same
/// rights installing it took.
pub fn release() -> Result<KeptNow, String> {
    let places = places()?;
    let released = stop_keeping(&places).map_err(|error| error.to_string())?;
    #[cfg(target_os = "linux")]
    let released = if matches!(released, Released::Command(_)) {
        match crate::linux_service::run(
            fastctx_install::service::Kind::Node,
            "uninstall",
            &places.root,
            &places.installed(),
        )? {
            Kept::Installed => Released::Removed,
            Kept::Declined => Released::Declined,
            other => return Err(format!("removing the node service came to {other:?}")),
        }
    } else {
        released
    };
    Ok(match released {
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
    if let Some(home) = &home {
        let host = match (codex, claude) {
            (true, false) => apply::Host::Codex,
            (false, true) => apply::Host::Claude,
            _ => return Err("a home directory requires exactly one host".into()),
        };
        places = places.at_home(host, home).map_err(|error| error.to_string())?;
    }
    if into || places.installed().is_file() {
        return hosts::run(&places, codex, claude, raise, into, home.as_deref());
    }
    let hosts = Hosts { codex, claude };
    let report = apply::unapply(&places, hosts).map_err(|error| error.to_string())?;
    // As in `hosts::run`: only what stayed as it was reaches the page; the
    // account of what was done is for the terminal and the log.
    Ok(report
        .notices
        .iter()
        .map(|notice| format!("{}: {}", notice.location, notice.reason))
        .chain(report.leftovers)
        .collect())
}
