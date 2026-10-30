//! The shell between the web front end and the World. It holds nothing of
//! its own: every command is one call of the core's client
//! (`fastctx-client`), the same layer the TUI and the commands use, and a
//! watched conversation streams the session's events to the page as they
//! come. The page speaks as the person at this machine, never as a session.

mod accounts;
mod book;
#[cfg(target_os = "android")]
mod android;
mod collab;
mod collaboration;
mod composer;
mod connection;
#[cfg(desktop)]
mod data;
mod held_events;
mod features;
mod goals;
mod hub_moving;
mod item_details;
mod kept;
mod layers;
#[cfg(desktop)]
mod local_repair;
mod memory;
mod plan;
mod profile;
#[cfg(desktop)]
mod repair_probe;
mod startup;
mod sentinels;
mod skills;
mod sentinel_controls;
mod transcript;
mod updates;
mod world_delta;
mod world_watch;

#[cfg(desktop)]
mod founding;
#[cfg(target_os = "linux")]
mod linux_service;
#[cfg(desktop)]
mod programs;
#[cfg(desktop)]
mod remote_world;
#[cfg(desktop)]
mod removal;
#[cfg(desktop)]
mod renewal;
#[cfg(desktop)]
mod retention;
#[cfg(desktop)]
mod setup;
#[cfg(desktop)]
mod terminal;
#[cfg(desktop)]
mod user_path;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

#[cfg(not(target_os = "android"))]
use fastctx_client::Conversation;
use fastctx_client::World;
use fastctx_types::file_search::{FileSearchReply, FileSearchRequest};
use fastctx_types::management::{Managed, Management};
use fastctx_types::project::{MapView, Record, RecordFields, Stamp};
use fastctx_types::residents::Landed;
use fastctx_types::session::control::{Control, Controlled, Offers};
use fastctx_types::town::{LookedUp, WorldWho};
use fastctx_types::{Address, AgentName, ResidentId, TownName};
use serde::Serialize;
use tauri::Manager as _;
use tauri::async_runtime::JoinHandle;
use tauri::ipc::Channel;

mod attachment_files;
mod binary_reading;
#[cfg(windows)]
mod desktop_presence;
mod folder_picker;
mod held_reads;
#[cfg(windows)]
mod installer_lines;
mod line_reading;
mod local_files;
mod logins;
mod names;
#[cfg(windows)]
mod package_removal;
mod pages;
mod reading;
mod save_as;
mod uploading;

#[cfg(windows)]
mod titlebar;

mod watch_delivery;
mod conversation_watch;

struct Shell {
    connection: connection::Connection,
    watches: Arc<Mutex<HashMap<u64, JoinHandle<()>>>>,
    /// What each watched conversation holds for its page.
    transcripts: Arc<transcript::Transcripts>,
    deliveries: watch_delivery::Deliveries,
    next: AtomicU64,
    page: AtomicU64,
    line_requests: line_reading::Requests,
    /// The file reads the page holds (`held_reads`).
    held_reads: held_reads::HeldReads,
}

impl Shell {
    fn world(&self) -> World {
        self.connection.world()
    }

    fn close_page(&self) {
        self.page.fetch_add(1, Ordering::Relaxed);
        self.world().close_file_searches();
        self.world().close_line_sources();
        self.line_requests.cancel_all();
        self.held_reads.release_all();
        for (_, watch) in self.watches.lock().expect("watch table").drain() {
            watch.abort();
        }
        self.transcripts.close_all();
        self.deliveries.close_all();
    }

    fn follow(
        &self,
        page: u64,
        id: u64,
        work: impl std::future::Future<Output = ()> + Send + 'static,
    ) -> Result<(), String> {
        let mut watches = self.watches.lock().expect("watch table");
        if self.page.load(Ordering::Relaxed) != page {
            return Err("the conversation's page closed".into());
        }
        let registry = Arc::downgrade(&self.watches);
        let task = tauri::async_runtime::spawn(async move {
            struct Finished {
                registry: std::sync::Weak<Mutex<HashMap<u64, JoinHandle<()>>>>,
                id: u64,
            }
            impl Drop for Finished {
                fn drop(&mut self) {
                    if let Some(registry) = self.registry.upgrade() {
                        registry.lock().expect("watch table").remove(&self.id);
                    }
                }
            }
            let _finished = Finished { registry, id };
            work.await;
        });
        watches.insert(id, task);
        Ok(())
    }
}

/// Available before joining and while no machines can answer.
#[tauri::command]
fn connection_state(shell: tauri::State<'_, Shell>) -> connection::State {
    shell.connection.state()
}

/// Desktop can use a local node or a device identity; mobile uses a device.
#[fastctx_gui_command::command]
async fn select_connection(
    shell: tauri::State<'_, Shell>,
    mode: connection::Mode,
) -> Result<connection::State, String> {
    let previous = shell.world();
    let (state, changed) = shell.connection.select(mode)?;
    if changed {
        previous.close_file_searches();
        previous.close_line_sources();
        shell.close_page();
    }
    Ok(state)
}

/// Everything that broke is tried again now: the page calls this when the
/// window comes to the front or the system says the network is back.
#[tauri::command]
fn reconnect(shell: tauri::State<'_, Shell>) {
    shell.world().retry_now();
}

/// The wait before try number `tries` of what the page asks of `machine`,
/// paced as the core paces everything that tries again
/// (`fastctx_client::again::pause`): `done` hears once when it is over,
/// which is early when the page is told to try now or a way to the machine
/// opens. `unwatch` with the id this returns calls it off.
#[fastctx_gui_command::command]
async fn pause(
    shell: tauri::State<'_, Shell>,
    tries: u32,
    machine_name: Option<String>,
    done: Channel<()>,
) -> Result<u64, String> {
    let page = shell.page.load(Ordering::Relaxed);
    let pause = fastctx_client::again::pause(&shell.world(), tries, machine(machine_name)?.as_ref());
    let id = shell.next.fetch_add(1, Ordering::Relaxed);
    shell.follow(page, id, async move {
        pause.await;
        let _ = done.send(());
    })?;
    Ok(id)
}

/// Native network and foreground-service callbacks use this after shell
/// setup. It interrupts an old dial/socket and the retry backoff immediately.
pub fn reconnect_device(app: &tauri::AppHandle) -> Result<(), String> {
    let shell = app.try_state::<Shell>().ok_or("the World shell has not started yet")?;
    shell.world().retry_now();
    Ok(())
}

/// One piece of a watched conversation, as the page receives it: the page
/// holds a part of it near its reader (`transcript.rs`).
#[derive(Clone, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
enum Piece {
    Resumed,
    /// Events kept after all the page was given, from `start` on.
    Extended {
        start: usize,
        events: Vec<Box<serde_json::value::RawValue>>,
    },
    /// The page's part is now this, in place of all it holds: the history
    /// as its machine has it now differs from what was given.
    Window {
        window: transcript::Window,
    },
    /// What came before the page's part arrived: what it holds stays, its
    /// places moved along by `offset` events and `turns_added` turns.
    Rebased {
        generation: u64,
        offset: usize,
        turns_added: usize,
    },
    /// The live part broke and is being read again: why, and since when
    /// (milliseconds since the Unix epoch).
    Interrupted {
        error: Option<String>,
        since: u64,
    },
    /// One thing as it happened, the `at`th event of the conversation.
    Event {
        at: usize,
        event: Box<serde_json::value::RawValue>,
    },
    /// Whether the conversation works now, and on which turn, as its
    /// machine holds it; told after every event before it. A machine too
    /// old to tell this never sends it.
    Now {
        busy: bool,
        turn: Option<String>,
        kept: Vec<fastctx_types::session::KeptWords>,
    },
    /// The watch is over: the node ended it, or the connection broke.
    Ended {
        error: Option<String>,
    },
    /// The conversation was deleted: nothing more of it comes.
    Deleted {
        error: String,
    },
    /// Its machine refused this account: watching again changes nothing
    /// until the account may see it.
    Refused {
        error: String,
    },
}

impl Piece {
    fn interrupted(interruption: &fastctx_client::Interruption) -> Self {
        Piece::Interrupted { error: Some(interruption.why.clone()), since: epoch_ms(interruption.since) }
    }

    /// How a watch that failed for `error` is over.
    fn failed(error: &fastctx_client::Error) -> Self {
        if error.is_deleted() {
            Piece::Deleted { error: error.to_string() }
        } else if error.is_refused() {
            Piece::Refused { error: error.to_string() }
        } else {
            Piece::Ended { error: Some(error.to_string()) }
        }
    }

    /// Whether nothing comes after it.
    fn last(&self) -> bool {
        matches!(self, Piece::Ended { .. } | Piece::Deleted { .. } | Piece::Refused { .. })
    }
}

/// Why a conversation could not be watched: `deleted` when it was deleted,
/// `refused` when its machine refused this account, so the page asks no
/// more.
#[derive(Serialize)]
pub(crate) struct NotWatched {
    message: String,
    deleted: bool,
    refused: bool,
}

impl From<fastctx_client::Error> for NotWatched {
    fn from(error: fastctx_client::Error) -> Self {
        Self { deleted: error.is_deleted(), refused: error.is_refused(), message: error.to_string() }
    }
}

impl NotWatched {
    /// How a watch that could not be opened again is over.
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    fn piece(self) -> Piece {
        if self.deleted {
            Piece::Deleted { error: self.message }
        } else if self.refused {
            Piece::Refused { error: self.message }
        } else {
            Piece::Ended { error: Some(self.message) }
        }
    }
}

impl From<String> for NotWatched {
    fn from(message: String) -> Self {
        Self { message, deleted: false, refused: false }
    }
}

/// A moment as milliseconds since the Unix epoch, as a page's `Date` takes
/// it.
pub(crate) fn epoch_ms(at: std::time::SystemTime) -> u64 {
    at.duration_since(std::time::UNIX_EPOCH).map(|since| since.as_millis() as u64).unwrap_or_default()
}

#[derive(Serialize)]
struct Watch {
    id: u64,
    window: transcript::Window,
}

#[derive(Serialize)]
struct Spoken {
    to: Address,
    landed: Landed,
    /// The words' id where they went, to know them among what that machine
    /// keeps (`Piece::Now`).
    delivery: Option<fastctx_types::NoticeId>,
}

/// Why words were not said: `again` when no answer came, so the page says
/// them again under the same id once it can.
#[derive(Serialize)]
pub(crate) struct NotSaid {
    pub(crate) message: String,
    pub(crate) again: bool,
}

fn resident(id: String) -> Result<ResidentId, String> {
    ResidentId::try_from(id).map_err(|error| error.to_string())
}

fn new_machine_name(name: Option<String>) -> Result<Option<fastctx_types::MachineName>, String> {
    name.map(|name| name.parse::<fastctx_types::MachineName>().map_err(|error| error.to_string())).transpose()
}

fn machine(name: Option<String>) -> Result<Option<fastctx_types::access::Written>, String> {
    name.map(|name| name.parse::<fastctx_types::access::Written>().map_err(|error| error.to_string())).transpose()
}

/// Starts watching a resident: returns the part of it the page opens on,
/// its latest turns, from what this device keeps of it at once where it
/// keeps it, and sends what follows down `pieces` until the page lets go.
/// The shell keeps the rest of it for the page to ask for
/// (`transcript.rs`).
#[fastctx_gui_command::command]
async fn watch(
    shell: tauri::State<'_, Shell>,
    resident_id: String,
    machine_name: Option<String>,
    pieces: Channel<watch_delivery::Wake>,
) -> Result<Watch, NotWatched> {
    let page = shell.page.load(Ordering::Relaxed);
    let world = shell.world();
    let resident = resident(resident_id)?;
    let machine = machine(machine_name)?;
    #[cfg(not(target_os = "android"))]
    let (conversation, history) = match &machine {
        Some(machine) => Conversation::open(&world, resident.clone(), machine.clone()).await,
        None => Conversation::watch(&world, resident.clone(), None).await,
    }?;
    #[cfg(target_os = "android")]
    let (conversation, history) = android::watching::open(&world, resident.clone(), machine.clone(), false).await?;
    let id = shell.next.fetch_add(1, Ordering::Relaxed);
    let (transcript, window) = transcript::Transcript::new(&history)?;
    drop(history);
    let transcript = shell.transcripts.open(id, transcript);
    let mut initial = Some(conversation);
    if let Err(error) = shell.follow_delivery(page, id, pieces, |piece| match piece {
        Piece::Now { .. } => Some("now".into()),
        _ => None,
    }, move |pieces, _| {
        conversation_watch::stream(initial.take(), world.clone(), resident.clone(), machine.clone(), transcript.clone(), pieces)
    }) {
        shell.transcripts.close(id);
        return Err(error.into());
    }
    Ok(Watch { id, window })
}

#[tauri::command]
fn unwatch(shell: tauri::State<'_, Shell>, id: u64) {
    if let Some(task) = shell.watches.lock().expect("watch table").remove(&id) {
        task.abort();
    }
    shell.transcripts.close(id);
    shell.deliveries.close(id);
}

/// The user's words to a resident, with the files uploaded for them; those
/// other than images go to `files_on` when the user chose a machine.
#[fastctx_gui_command::command]
async fn talk(
    shell: tauri::State<'_, Shell>,
    resident_id: String,
    machine_name: Option<String>,
    text: String,
    attachments: Option<Vec<fastctx_types::attachments::Attachment>>,
    files_on: Option<String>,
    id: Option<String>,
) -> Result<Spoken, NotSaid> {
    let refused = |message: String| NotSaid { message, again: false };
    let files_on = files_on
        .map(|written| written.parse::<fastctx_types::access::Written>().map_err(|error| error.to_string()))
        .transpose()
        .map_err(refused)?;
    let said = shell
        .world()
        .talk_with_attachments(
            resident(resident_id).map_err(refused)?,
            machine(machine_name).map_err(refused)?,
            text,
            attachments.unwrap_or_default(),
            files_on,
            id,
        )
        .await
        // Words a busy machine did not take go in once it has room, as words
        // no answer came to do.
        .map_err(|error| NotSaid {
            message: error.to_string(),
            again: World::unanswered(&error) || World::busy(&error),
        })?;
    Ok(Spoken { to: said.to, landed: said.landed, delivery: said.delivery })
}

/// Carries a file on this computer to a machine, for words to an agent
/// there or to one about to be asked in; `sent` hears how much has gone.
#[fastctx_gui_command::command]
async fn upload_path(
    shell: tauri::State<'_, Shell>,
    target: fastctx_types::attachments::UploadTarget,
    machine_name: Option<String>,
    path: String,
    sent: Channel<u64>,
) -> Result<fastctx_types::attachments::Attachment, String> {
    let world = shell.world();
    let machine = machine(machine_name)?;
    #[cfg(target_os = "android")]
    android::readiness::wait(&world, machine.as_ref()).await?;
    uploading::from_path(&world, target, machine, std::path::Path::new(&path), sent).await
}

/// Carries bytes the page holds to a machine. The bytes are the body; whom
/// they are for (the target as JSON) and what they are come as headers,
/// each percent-encoded, since a name may be in any language.
#[fastctx_gui_command::command]
async fn upload_bytes(
    shell: tauri::State<'_, Shell>,
    request: tauri::ipc::Request<'_>,
) -> Result<fastctx_types::attachments::Attachment, String> {
    let header = |name: &str| -> Result<String, String> {
        let raw = request.headers().get(name).ok_or(format!("the upload names no {name}"))?;
        decoded(raw.to_str().map_err(|error| error.to_string())?)
    };
    #[cfg(target_os = "android")]
    let bytes = android::uploading::bytes(request.body())?;
    #[cfg(not(target_os = "android"))]
    let tauri::ipc::InvokeBody::Raw(bytes) = request.body() else {
        return Err("the attachment's bytes did not come as bytes".into());
    };
    let target: fastctx_types::attachments::UploadTarget =
        serde_json::from_str(&header("target")?).map_err(|error| format!("the upload's target: {error}"))?;
    let machine = machine(Some(header("machine")?).filter(|name| !name.is_empty()))?;
    #[cfg(not(target_os = "android"))]
    let bytes = bytes.clone();
    let world = shell.world();
    #[cfg(target_os = "android")]
    android::readiness::wait(&world, machine.as_ref()).await?;
    uploading::from_bytes(&world, target, machine, header("name")?, header("type")?, bytes).await
}

/// Undoes `encodeURIComponent`.
fn decoded(text: &str) -> Result<String, String> {
    let mut bytes = Vec::with_capacity(text.len());
    let mut rest = text.as_bytes();
    while let Some((&first, tail)) = rest.split_first() {
        if first == b'%' && tail.len() >= 2 {
            let pair = std::str::from_utf8(&tail[..2]).map_err(|error| error.to_string())?;
            bytes.push(u8::from_str_radix(pair, 16).map_err(|error| format!("%{pair}: {error}"))?);
            rest = &tail[2..];
        } else {
            bytes.push(first);
            rest = tail;
        }
    }
    String::from_utf8(bytes).map_err(|error| error.to_string())
}

/// A project's overview: its map, and who is in its town now.
#[derive(Serialize)]
struct Project {
    map: MapView,
    who: WorldWho,
}

#[fastctx_gui_command::command]
async fn project(shell: tauri::State<'_, Shell>, town: String) -> Result<Project, String> {
    let town = TownName::try_from(town).map_err(|error| error.to_string())?;
    let world = shell.world();
    let (map, who) = tokio::join!(world.map(Some(town.clone())), world.who(Some(town), true));
    Ok(Project { map: map.map_err(|error| error.to_string())?, who: who.map_err(|error| error.to_string())? })
}

/// Asks an agent into the World for the user: on a machine, in a town,
/// with the user's words as the first it hears.
// Each argument is a named field of the page's call; a struct would only
// nest them one level deeper there.
#[allow(clippy::too_many_arguments)]
#[fastctx_gui_command::command]
async fn open(
    shell: tauri::State<'_, Shell>,
    agent: String,
    town: Option<String>,
    machine_name: Option<String>,
    text: String,
    model: Option<String>,
    effort: Option<String>,
    attachments: Option<Vec<fastctx_types::attachments::Attachment>>,
    choices: Option<fastctx_types::agents::Choices>,
) -> Result<Spoken, String> {
    let agent = AgentName::try_from(agent.as_str()).map_err(|error| error.to_string())?;
    let town = town.map(|town| TownName::try_from(town).map_err(|error| error.to_string())).transpose()?;
    let options = fastctx_types::agents::SessionOptions { model, reasoning_effort: effort, ..Default::default() };
    let (attachments, choices) = (attachments.unwrap_or_default(), choices.unwrap_or_default());
    let said = shell
        .world()
        .open_with_attachments(agent, town, machine(machine_name)?, text, options, attachments, choices)
        .await
        .map_err(|error| error.to_string())?;
    Ok(Spoken { to: said.to, landed: said.landed, delivery: said.delivery })
}

/// What happened in the World after a moment, as each machine's records
/// say now; the whole World, or one project's machines.
#[fastctx_gui_command::command]
async fn since(
    shell: tauri::State<'_, Shell>,
    since: String,
    town: Option<String>,
) -> Result<fastctx_types::since::WorldSince, String> {
    let since: jiff::Timestamp = since.parse().map_err(|error: jiff::Error| error.to_string())?;
    let town = town.map(|town| TownName::try_from(town).map_err(|error| error.to_string())).transpose()?;
    shell.world().since(since, town).await.map_err(|error| error.to_string())
}

/// The agents each machine can start, as their official programs say:
/// one machine, or every machine in the World.
#[fastctx_gui_command::command]
async fn agents(
    shell: tauri::State<'_, Shell>,
    machine_name: Option<String>,
) -> Result<fastctx_types::agents::WorldAgents, String> {
    shell.world().agents(machine(machine_name)?).await.map_err(|error| error.to_string())
}

/// Stops the turn a resident is running; says whether one was.
#[fastctx_gui_command::command]
async fn stop(
    shell: tauri::State<'_, Shell>,
    resident_id: String,
    machine_name: Option<String>,
) -> Result<bool, String> {
    let (_, stopped) =
        shell.world().stop(resident(resident_id)?, machine(machine_name)?).await.map_err(|error| error.to_string())?;
    Ok(stopped)
}

/// Stops a job the user chose, on the machine it runs on: the World tells
/// the agent that started it that the user stopped it.
#[fastctx_gui_command::command]
async fn stop_job(shell: tauri::State<'_, Shell>, job: String, machine_name: Option<String>) -> Result<(), String> {
    let job = job.parse().map_err(|error| format!("{error}"))?;
    shell.world().stop_job(job, machine(machine_name)?).await.map_err(|error| error.to_string())
}

/// Names of files for an `@` in the user's words, found on `machine_name`
/// (this node when absent) in the places the request's scope allows.
#[fastctx_gui_command::command]
async fn find_files(
    shell: tauri::State<'_, Shell>,
    machine_name: Option<String>,
    request: FileSearchRequest,
) -> Result<FileSearchReply, String> {
    shell.world().find_files(machine(machine_name)?, request).await.map_err(|error| error.to_string())
}

/// The folders in a folder of `machine_name` (this node when absent), for
/// pointing a conversation at some; with no `path`, where a choice starts.
/// A failure keeps its code: a folder macOS keeps private is said so.
#[fastctx_gui_command::command]
async fn folders(
    shell: tauri::State<'_, Shell>,
    machine_name: Option<String>,
    path: Option<String>,
) -> Result<fastctx_types::focus::Folders, fastctx_types::ipc::Failure> {
    let machine = machine(machine_name).map_err(fastctx_types::ipc::Failure::new)?;
    shell.world().folders(machine, path).await.map_err(|error| match error {
        fastctx_client::world::Error::Call(fastctx_client::CallError::Failed(failure)) => failure,
        other => fastctx_types::ipc::Failure::new(other.to_string()),
    })
}

/// Where a Mac's user lets Fastctx into what macOS keeps private: System
/// Settings at Full Disk Access, and the World program shown in the Finder
/// beside it, to be put on that list (`world-gui.md`, 设置).
#[fastctx_gui_command::command]
async fn allow_privacy(app: tauri::AppHandle) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        use tauri_plugin_opener::OpenerExt as _;
        let program = fastctx_platform::fastctx_root().map_err(|error| error.to_string())?.join("bin").join("fastctx");
        std::process::Command::new("/usr/bin/open")
            .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles")
            .status()
            .map_err(|error| error.to_string())?;
        app.opener().reveal_item_in_dir(program).map_err(|error| error.to_string())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
        Err("only macOS keeps folders private from a program this way".to_string())
    }
}

/// How `machine_name` (this computer when absent) is doing, for its details
/// page; its lines' history only after `after`, when given.
#[fastctx_gui_command::command]
async fn machine_stats(
    shell: tauri::State<'_, Shell>,
    machine_name: Option<String>,
    after: Option<jiff::Timestamp>,
    seen: Option<fastctx_types::stats::StatsSeen>,
) -> Result<fastctx_types::stats::MachineStats, String> {
    shell.world().stats(machine(machine_name)?, after, seen).await.map_err(|error| error.to_string())
}

/// What `machine_name` (this computer when absent) keeps free in memory for
/// its agents; sets it first when `bytes` is given.
#[fastctx_gui_command::command]
async fn machine_memory_room(
    shell: tauri::State<'_, Shell>,
    machine_name: Option<String>,
    bytes: Option<u64>,
) -> Result<u64, String> {
    shell.world().memory_room(machine(machine_name)?, bytes).await.map_err(|error| error.to_string())
}

/// Archives a conversation or brings it back, lets it sleep, wakes it or
/// moves it to another project, or points it at machines and folders, as
/// the user; each stage is answered as it went.
#[fastctx_gui_command::command]
async fn manage(
    shell: tauri::State<'_, Shell>,
    resident_id: String,
    machine_name: Option<String>,
    action: Management,
) -> Result<Managed, String> {
    let (_, managed) = shell
        .world()
        .manage(resident(resident_id)?, machine(machine_name)?, action)
        .await
        .map_err(|error| error.to_string())?;
    Ok(managed)
}

/// Deletes a conversation from every machine and device keeping something
/// of it (`world-colleagues.md`, 用户在任何地方和居民说话).
#[fastctx_gui_command::command]
async fn forget_conversation(
    shell: tauri::State<'_, Shell>,
    resident_id: String,
    machine_name: String,
) -> Result<fastctx_types::management::Forgetting, String> {
    let machine = machine(Some(machine_name))?.ok_or("a conversation's machine is named")?;
    shell.world().forget_conversation(resident(resident_id)?, machine).await.map_err(|error| error.to_string())
}

/// What Claude Code on a machine records of the user's consent to running
/// the World's sessions without approvals, after the user's word on it.
#[derive(serde::Serialize)]
struct ClaudeConsent {
    accepted: bool,
    changed: bool,
}

/// The user's word, given in the interface, on Claude Code's
/// bypass-permissions consent on one machine.
#[fastctx_gui_command::command]
async fn claude_consent(
    shell: tauri::State<'_, Shell>,
    machine_name: String,
    accept: bool,
    home: Option<std::path::PathBuf>,
) -> Result<ClaudeConsent, String> {
    let machine = machine_name.parse().map_err(|error| format!("{error}"))?;
    let (accepted, changed) =
        shell.world().claude_consent(machine, accept, home).await.map_err(|error| error.to_string())?;
    Ok(ClaudeConsent { accepted, changed })
}

/// Does something to a resident's conversation for the user, besides
/// speaking into it.
#[fastctx_gui_command::command]
async fn control(
    shell: tauri::State<'_, Shell>,
    resident_id: String,
    machine_name: Option<String>,
    control: Control,
) -> Result<Controlled, String> {
    let (_, controlled) = shell
        .world()
        .control(resident(resident_id)?, machine(machine_name)?, control)
        .await
        .map_err(|error| error.to_string())?;
    Ok(controlled)
}

/// What a resident's conversation offers now.
#[fastctx_gui_command::command]
async fn offers(
    shell: tauri::State<'_, Shell>,
    resident_id: String,
    machine_name: Option<String>,
) -> Result<Offers, String> {
    let (_, offers) = shell
        .world()
        .offers(resident(resident_id)?, machine(machine_name)?)
        .await
        .map_err(|error| error.to_string())?;
    Ok(offers)
}

/// A text file on a machine, or a piece of it, read for the user. A read
/// goes on through breaks; the page names it `read` to let go of it, and
/// is told how it goes down `progress` (`held_reads`).
#[fastctx_gui_command::command]
async fn read_text(
    shell: tauri::State<'_, Shell>,
    machine_name: Option<String>,
    path: String,
    range: fastctx_types::file_read::FileRange,
    read: Option<String>,
    progress: Option<tauri::ipc::JavaScriptChannelId>,
    webview: tauri::Webview,
) -> Result<reading::TextRead, String> {
    let machine = machine(machine_name)?;
    let world = shell.world();
    let reading = reading::read_text(&world, machine, path, range, held_reads::channel(&webview, progress));
    shell.held_reads.hold(read, reading).await.unwrap_or_else(|| Err(held_reads::LET_GO.into()))
}

/// A file on a machine, whole, as bytes for the page to show as it is.
#[fastctx_gui_command::command]
async fn read_bytes(
    shell: tauri::State<'_, Shell>,
    machine_name: Option<String>,
    path: String,
    read: Option<String>,
    progress: Option<tauri::ipc::JavaScriptChannelId>,
    webview: tauri::Webview,
) -> Result<reading::BytesRead, String> {
    let machine = machine(machine_name)?;
    let world = shell.world();
    let reading = reading::read_bytes(&world, machine, path, held_reads::channel(&webview, progress));
    shell.held_reads.hold(read, reading).await.unwrap_or_else(|| Err(held_reads::LET_GO.into()))
}

/// Streams an original file into this device's cache for an external app.
#[fastctx_gui_command::command]
async fn fetch_file(
    app: tauri::AppHandle,
    shell: tauri::State<'_, Shell>,
    machine_name: Option<String>,
    path: String,
    read: Option<String>,
    progress: Option<tauri::ipc::JavaScriptChannelId>,
    webview: tauri::Webview,
) -> Result<String, String> {
    let world = shell.world();
    let cache = match world.cache() {
        Some(cache) => cache.root().join("files"),
        None => app.path().app_cache_dir().map_err(|error| error.to_string())?,
    };
    let machine = machine(machine_name)?;
    let fetching = local_files::fetch(&world, machine, path, &cache, held_reads::channel(&webview, progress));
    shell.held_reads.hold(read, fetching).await.unwrap_or_else(|| Err(held_reads::LET_GO.into()))
}

/// Lets go of a file read the page no longer wants.
#[tauri::command]
fn cancel_read(shell: tauri::State<'_, Shell>, read: String) {
    shell.held_reads.release(&read);
}

#[fastctx_gui_command::command]
async fn thumbnail(
    shell: tauri::State<'_, Shell>,
    machine_name: Option<String>,
    path: String,
    max: u32,
) -> Result<fastctx_types::thumbnail::Thumbnail, String> {
    shell
        .world()
        .thumbnail(machine(machine_name)?, fastctx_types::thumbnail::ThumbnailRequest { path, max })
        .await
        .map_err(|error| error.to_string())
}

#[fastctx_gui_command::command]
async fn thumbnail_attachment(
    shell: tauri::State<'_, Shell>,
    attachment: fastctx_types::attachments::AttachmentSource,
    max: u32,
) -> Result<fastctx_types::thumbnail::Thumbnail, attachment_files::Failure> {
    shell.world().thumbnail_attachment(attachment, max).await.map_err(attachment_files::Failure::from)
}

#[fastctx_gui_command::command]
async fn fetch_attachment(
    app: tauri::AppHandle,
    shell: tauri::State<'_, Shell>,
    attachment: fastctx_types::attachments::AttachmentSource,
    read: Option<String>,
    progress: Option<tauri::ipc::JavaScriptChannelId>,
    webview: tauri::Webview,
) -> Result<attachment_files::Fetched, attachment_files::Failure> {
    let world = shell.world();
    let fetching = async {
        // Kept by its bytes where this device keeps what it views: opened
        // again, it is not fetched again.
        let mut progress = held_reads::channel(&webview, progress);
        let kept = world
            .kept_original(attachment.clone(), |reading| held_reads::tell(reading, progress.take()))
            .await
            .map_err(attachment_files::Failure::from)?;
        if let Some((path, file)) = kept {
            return Ok(attachment_files::Fetched { path: path.to_string_lossy().into_owned(), file });
        }
        let cache = app.path().app_cache_dir().map_err(|error| attachment_files::Failure::failed(error.to_string()))?;
        attachment_files::fetch(&world, attachment, &cache, progress).await
    };
    shell
        .held_reads
        .hold(read, fetching)
        .await
        .unwrap_or_else(|| Err(attachment_files::Failure::failed(held_reads::LET_GO.into())))
}

/// What opening a file on this device came to: it opened, or it is a
/// program, which runs when opened, and the page asks first (`programs`).
#[derive(serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Opened {
    Opened,
    /// A phone opens a file in the app the system picks, which runs nothing.
    #[cfg_attr(target_os = "android", allow(dead_code))]
    Program,
}

/// Opens a file on this device with the system's own handler; a program
/// only when `run` says the user chose to run it.
#[fastctx_gui_command::command]
async fn open_local(app: tauri::AppHandle, path: String, run: Option<bool>) -> Result<Opened, String> {
    #[cfg(target_os = "android")]
    {
        let _ = run;
        android::open_file(&app, path).await.map(|()| Opened::Opened)
    }
    #[cfg(not(target_os = "android"))]
    {
        use tauri_plugin_opener::OpenerExt as _;
        if run != Some(true) && programs::runs(std::path::Path::new(&path)) {
            return Ok(Opened::Program);
        }
        app.opener().open_path(path, None::<&str>).map(|()| Opened::Opened).map_err(|error| error.to_string())
    }
}

/// Select a destination on this device and copy a local file there.
#[fastctx_gui_command::command]
async fn save_as(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    path: String,
    suggested_name: String,
) -> save_as::Outcome {
    save_as::save(app, window, path, suggested_name).await
}

/// The system's own folder picker on this computer; none when the user
/// cancelled it.
#[fastctx_gui_command::command]
async fn pick_folder(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
) -> Result<Option<std::path::PathBuf>, String> {
    folder_picker::pick(app, window).await
}

#[fastctx_gui_command::command]
async fn reveal(app: tauri::AppHandle, path: String) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt as _;
    app.opener().reveal_item_in_dir(path).map_err(|error| error.to_string())
}

/// A file's metadata and bytes in one binary IPC response.
#[fastctx_gui_command::command]
async fn read_binary(
    shell: tauri::State<'_, Shell>,
    machine_name: Option<String>,
    path: String,
    read: Option<String>,
    progress: Option<tauri::ipc::JavaScriptChannelId>,
    webview: tauri::Webview,
) -> Result<tauri::ipc::Response, String> {
    let machine = machine(machine_name)?;
    let world = shell.world();
    let reading = binary_reading::read_binary(&world, machine, path, held_reads::channel(&webview, progress));
    shell.held_reads.hold(read, reading).await.unwrap_or_else(|| Err(held_reads::LET_GO.into()))
}

/// Residents or jobs in detail, by id, from every machine that has them.
#[fastctx_gui_command::command]
async fn look(shell: tauri::State<'_, Shell>, ids: Vec<String>) -> Result<Vec<LookedUp>, String> {
    shell.world().look(ids).await.map_err(|error| error.to_string())
}

/// A version of a town's record, as the World stamped it.
#[derive(Serialize)]
struct TownRecord {
    town: TownName,
    record: Record,
    opened: Stamp,
}

fn town(name: String) -> Result<TownName, String> {
    TownName::try_from(name).map_err(|error| error.to_string())
}

/// Opens a project with the first version of its record, in the account
/// named, of the hub named, or in the one account this computer is in.
#[fastctx_gui_command::command]
async fn project_new(
    shell: tauri::State<'_, Shell>,
    name: String,
    fields: RecordFields,
    account_name: Option<String>,
    hub_name: Option<String>,
) -> Result<TownRecord, String> {
    let account =
        account_name.map(fastctx_types::AccountName::try_from).transpose().map_err(|error| error.to_string())?;
    let hub = hub_name.map(fastctx_types::HubName::try_from).transpose().map_err(|error| error.to_string())?;
    let made = shell.world().open_town(town(name)?, fields, account, hub).await.map_err(|error| error.to_string())?;
    Ok(TownRecord { town: made.town, record: made.record, opened: made.opened })
}

/// Writes the next version of a project's record; what is not given stays.
#[fastctx_gui_command::command]
async fn project_edit(
    shell: tauri::State<'_, Shell>,
    name: String,
    fields: RecordFields,
) -> Result<TownRecord, String> {
    let made = shell.world().edit_town(Some(town(name)?), fields).await.map_err(|error| error.to_string())?;
    Ok(TownRecord { town: made.town, record: made.record, opened: made.opened })
}

/// Opens a resident on this computer in its own official interface, in a
/// terminal window of its own; resolves when that window's attach ends.
#[cfg(desktop)]
#[fastctx_gui_command::command]
async fn attach_here(shell: tauri::State<'_, Shell>, resident_id: String) -> Result<(), String> {
    let program = shell.connection.local_program()?;
    let resident = resident(resident_id)?;
    tauri::async_runtime::spawn_blocking(move || terminal::attach(&program, &resident))
        .await
        .map_err(|error| error.to_string())?
}

/// Where this computer stands with the World.
#[cfg(desktop)]
#[fastctx_gui_command::command]
async fn setup_state(shell: tauri::State<'_, Shell>) -> Result<setup::Setup, String> {
    shell.connection.local_program()?;
    tauri::async_runtime::spawn_blocking(setup::setup).await.map_err(|error| error.to_string())?
}

/// Puts the fastctx this application carries in place for the hosts.
#[cfg(desktop)]
#[fastctx_gui_command::command]
async fn place_fastctx(shell: tauri::State<'_, Shell>) -> Result<String, String> {
    shell.connection.local_program()?;
    tauri::async_runtime::spawn_blocking(setup::place).await.map_err(|error| error.to_string())?
}

#[cfg(desktop)]
#[fastctx_gui_command::command]
async fn renewal_state(shell: tauri::State<'_, Shell>) -> Result<fastctx_types::renewal::Status, String> {
    shell.connection.local_program()?;
    tauri::async_runtime::spawn_blocking(renewal::state).await.map_err(|error| error.to_string())?
}

#[cfg(desktop)]
#[fastctx_gui_command::command]
async fn renew_here(shell: tauri::State<'_, Shell>) -> Result<(), String> {
    shell.connection.local_program()?;
    tauri::async_runtime::spawn_blocking(renewal::now).await.map_err(|error| error.to_string())?
}

/// Has a system service keep this computer's node, asking once for
/// administrator rights.
#[cfg(desktop)]
#[fastctx_gui_command::command]
async fn keep_here(shell: tauri::State<'_, Shell>) -> Result<setup::KeptNow, String> {
    // A computer that joined as a device has no node to keep.
    shell.connection.local_program()?;
    setup::keep(&shell.world()).await
}

/// Removes the service that keeps this computer's node.
#[cfg(desktop)]
#[fastctx_gui_command::command]
async fn release_here() -> Result<setup::KeptNow, String> {
    tauri::async_runtime::spawn_blocking(setup::release).await.map_err(|error| error.to_string())?
}

/// Puts the World's tools into this computer's hosts, or takes them out.
#[cfg(desktop)]
#[fastctx_gui_command::command]
async fn apply_hosts(
    shell: tauri::State<'_, Shell>,
    codex: bool,
    claude: bool,
    raise: bool,
    into: bool,
    home: Option<std::path::PathBuf>,
) -> Result<Vec<String>, String> {
    shell.connection.local_program()?;
    tauri::async_runtime::spawn_blocking(move || setup::hosts(codex, claude, raise, into, home))
        .await
        .map_err(|error| error.to_string())?
}

/// What of the window is kept from one run to the next: where it was and
/// how large; never whether it shows or how it is framed, which the shell
/// decides.
#[cfg(desktop)]
pub(crate) const WINDOW_KEPT: tauri_plugin_window_state::StateFlags = tauri_plugin_window_state::StateFlags::SIZE
    .union(tauri_plugin_window_state::StateFlags::POSITION)
    .union(tauri_plugin_window_state::StateFlags::MAXIMIZED);

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    #[cfg(desktop)]
    {
        let args: Vec<_> = std::env::args_os().collect();
        if args.get(1).is_some_and(|arg| arg == "--repair") {
            if let Err(error) = local_repair::launch(None, true) {
                startup::persist("launch_repair", &error);
                std::process::exit(1);
            }
            return;
        }
        if args.get(1).is_some_and(|arg| arg == "--repair-probe") {
            let result = match (args.get(2), args.get(3)) {
                (Some(receipt), Some(profile)) => repair_probe::run(receipt.into(), profile.into()),
                _ => Err("repair probe needs a receipt and an isolated profile".into()),
            };
            if let Err(error) = result {
                eprintln!("{error}");
                std::process::exit(1);
            }
            return;
        }
    }
    #[cfg(all(windows, not(feature = "page-sandbox-probe")))]
    let (desktop, desktop_error) = match desktop_presence::enter() {
        Ok(Some(instance)) => (Some(instance), None),
        Ok(None) => return,
        Err(error) => (None, Some(error)),
    };
    let builder = tauri::Builder::default()
        .manage(startup::Startup::default())
        .plugin(tauri_plugin_world_pages::init())
        // Its script also sends every link that opens a new window to the
        // system's browser, where the webview alone would drop it.
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            #[cfg(desktop)]
            app.manage(removal::State::default());
            #[cfg(desktop)]
            app.manage(remote_world::Sessions::default());
            if let Some(window) = app.get_webview_window("main") {
                #[cfg(windows)]
                if window.hwnd().is_ok_and(|handle| unsafe {
                    !windows::Win32::UI::WindowsAndMessaging::IsWindowVisible(windows::Win32::Foundation::HWND(
                        handle.0,
                    ))
                    .as_bool()
                }) {
                    // A hidden launcher can override the first ShowWindow call;
                    // reset tao's visibility cache before asking it to show again.
                    if let Err(error) = window.hide() {
                        startup::warning(app.handle(), "show_window", error.to_string());
                    }
                }
                if let Err(error) = window.show() {
                    startup::warning(app.handle(), "show_window", error.to_string());
                }
            }
            startup::open(app.handle().clone());
            Ok(())
        })
        .on_page_load(|webview, payload| {
            if webview.label() == "main"
                && payload.event() == tauri::webview::PageLoadEvent::Started
                && let Some(shell) = webview.app_handle().try_state::<Shell>()
            {
                shell.close_page();
            }
        })
        .invoke_handler(tauri::generate_handler![
            startup::startup_state,
            startup::startup_retry,
            #[cfg(desktop)]
            local_repair::repair_here,
            #[cfg(desktop)]
            local_repair::repair_state,
            updates::update_state,
            updates::update_repair,
            updates::update_progress,
            updates::update_look,
            updates::update_set,
            updates::update_fetch,
            updates::update_choose,
            updates::update_machine,
            updates::machine_updates,
            updates::network_updates,
            updates::update_install,
            #[cfg(desktop)]
            removal::removal_plan,
            #[cfg(desktop)]
            removal::removal_run,
            #[cfg(desktop)]
            removal::removal_finish,
            #[cfg(desktop)]
            remote_world::remote_probe,
            #[cfg(desktop)]
            remote_world::remote_look,
            #[cfg(desktop)]
            remote_world::remote_hub,
            #[cfg(desktop)]
            remote_world::remote_open,
            #[cfg(desktop)]
            remote_world::remote_join,
            #[cfg(desktop)]
            remote_world::remote_arrive,
            #[cfg(desktop)]
            remote_world::remote_close,
            #[cfg(desktop)]
            remote_world::remote_withdraw,
            #[cfg(desktop)]
            retention::retention_read,
            #[cfg(desktop)]
            retention::retention_write,
            connection_state,
            select_connection,
            reconnect,
            pause,
            names::name_problem,
            accounts::login,
            accounts::login_token,
            accounts::peek_token,
            accounts::open_account,
            accounts::accounts,
            accounts::seen_dissolved,
            accounts::machine_log,
            plan::plan_read,
            plan::plan_write,
            plan::plan_watch,
            sentinel_controls::sentinel_list,
            sentinel_controls::sentinel_stop,
            sentinel_controls::sentinel_record,
            sentinel_controls::sentinel_overview,
            sentinels::sentinel_settings_read,
            skills::skills_list,
            skills::skill_read,
            skills::skill_save,
            skills::skill_delete,
            sentinels::sentinel_settings_write,
            sentinels::sentinel_key_set,
            profile::profile_read,
            profile::profile_subagents,
            profile::profile_write,
            profile::mcp,
            collab::collab_read,
            collab::collab_write,
            collaboration::collaboration_tree,
            memory::memory_list,
            memory::memory_read,
            memory::memory_save_spec,
            memory::memory_add_journal,
            memory::memory_settings_read,
            memory::memory_settings_set,
            memory::memory_watch,
            layers::layer_read,
            layers::layer_set,
            layers::bring_in,
            book::book,
            features::features_read,
            features::features_set,
            features::features_language,
            features::features_watch,
            #[cfg(desktop)]
            data::data,
            accounts::account_change,
            accounts::server_command,
            hub_moving::hub_move,
            #[cfg(desktop)]
            hub_moving::arrive_hub,
            accounts::agent_homes,
            logins::agent_logins,
            logins::scan_agents,
            logins::add_agent_login,
            logins::agent_folder,
            logins::preview_agent_change,
            logins::change_agent_login,
            logins::read_agent_limits,
            logins::set_agent_title,
            logins::agent_usage,
            logins::agent_calls,
            world_watch::world_now,
            world_watch::world_watch,
            since,
            agents,
            project,
            project_new,
            project_edit,
            watch,
            unwatch,
            watch_delivery::watch_delivery_read,
            transcript::conversation_part,
            transcript::conversation_cut,
            transcript::conversation_locate,
            transcript::conversation_index,
            transcript::conversation_find,
            kept::kept_usage,
            kept::kept_clear,
            kept::kept_forget,
            item_details::item_detail,
            item_details::agent_activity,
            goals::goal_read,
            goals::goal_history,
            goals::goal_past,
            goals::goal_update,
            goals::goal_watch,
            composer::composer_read,
            composer::composer_watch,
            composer::composer_update,
            composer::composer_send,
            composer::composer_attach_path,
            composer::composer_attach_bytes,
            talk,
            upload_path,
            upload_bytes,
            uploading::attachment_places,
            open,
            stop,
            stop_job,
            manage,
            forget_conversation,
            claude_consent,
            find_files,
            folders,
            machine_stats,
            machine_memory_room,
            accounts::rename_here,
            accounts::hub_proxy,
            pages::page_read,
            pages::page_history,
            pages::page_consent,
            pages::page_act,
            pages::page_withdraw,
            pages::page_restore,
            look,
            read_text,
            read_bytes,
            fetch_file,
            thumbnail,
            thumbnail_attachment,
            fetch_attachment,
            line_reading::lines_open,
            line_reading::lines_read,
            line_reading::lines_read_projected,
            line_reading::lines_find,
            line_reading::lines_close,
            line_reading::cancel_line_request,
            cancel_read,
            open_local,
            save_as,
            pick_folder,
            reveal,
            allow_privacy,
            read_binary,
            control,
            offers,
            #[cfg(desktop)]
            setup_state,
            #[cfg(desktop)]
            place_fastctx,
            #[cfg(desktop)]
            renewal_state,
            #[cfg(desktop)]
            renew_here,
            #[cfg(desktop)]
            #[cfg(desktop)]
            apply_hosts,
            #[cfg(desktop)]
            attach_here,
            #[cfg(desktop)]
            keep_here,
            #[cfg(desktop)]
            release_here,
            #[cfg(desktop)]
            founding::here_addresses,
            #[cfg(desktop)]
            founding::create_world
        ]);
    #[cfg(desktop)]
    let builder = builder
        .plugin(tauri_plugin_window_state::Builder::default().with_state_flags(WINDOW_KEPT).build())
        .plugin(tauri_plugin_updater::Builder::new().build());
    #[cfg(all(windows, not(feature = "page-sandbox-probe")))]
    let builder = builder.plugin(desktop_presence::plugin(desktop, desktop_error));
    #[cfg(windows)]
    let builder = builder.plugin(titlebar::plugin());
    #[cfg(target_os = "android")]
    let builder = builder.plugin(android::plugin());
    let context = tauri::generate_context!();
    #[cfg(windows)]
    let context = titlebar::configure(context);
    if let Err(error) = builder.run(context) {
        startup::persist("open_window", &error.to_string());
    }
}
