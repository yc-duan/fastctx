//! The shell between the web front end and the World. It holds nothing of
//! its own: every command is one call of the core's client
//! (`fastctx-client`), the same layer the TUI and the commands use, and a
//! watched conversation streams the session's events to the page as they
//! come. The page speaks as the person at this machine, never as a session.

mod accounts;
#[cfg(target_os = "android")]
mod android;
mod collab;
mod composer;
mod connection;
mod features;

#[cfg(desktop)]
mod founding;
#[cfg(target_os = "linux")]
mod linux_service;
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
use fastctx_types::project::{JournalSummary, JournalView, MapView, Record, RecordFields, Stamp, TownSummary};
use fastctx_types::residents::Landed;
use fastctx_types::session::SessionEvent;
use fastctx_types::session::control::{Control, Controlled, Offers};
use fastctx_types::town::{LookedUp, WhoScope, WorldWho};
use fastctx_types::world::NodesReply;
use fastctx_types::{Address, AgentName, EntryId, JournalName, MachineName, ResidentId, TownName};
use serde::Serialize;
use tauri::Manager as _;
use tauri::async_runtime::JoinHandle;
use tauri::ipc::Channel;

mod attachment_files;
mod binary_reading;
#[cfg(windows)]
mod desktop_presence;
mod line_reading;
mod local_files;
mod pages;
mod reading;
mod save_as;
mod uploading;

#[cfg(windows)]
mod titlebar;

struct Shell {
    connection: connection::Connection,
    watches: Arc<Mutex<HashMap<u64, JoinHandle<()>>>>,
    next: AtomicU64,
    page: AtomicU64,
    line_requests: line_reading::Requests,
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
        for (_, watch) in self.watches.lock().expect("watch table").drain() {
            watch.abort();
        }
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

/// Also callable by the native application's network/resume callbacks.
#[tauri::command]
fn reconnect(shell: tauri::State<'_, Shell>) -> Result<(), String> {
    reconnect_world(&shell.world())
}

/// Native network and foreground-service callbacks use this after shell
/// setup. It interrupts an old dial/socket and the retry backoff immediately.
pub fn reconnect_device(app: &tauri::AppHandle) -> Result<(), String> {
    let shell = app.try_state::<Shell>().ok_or("the World shell has not started yet")?;
    reconnect_world(&shell.world())
}

fn reconnect_world(world: &World) -> Result<(), String> {
    let device = world.device().ok_or("this interface uses a local node, not a device connection")?;
    device.reconnect();
    Ok(())
}

/// The World as the front page reads it, asked in one go: who is where, the
/// machines hub by hub, the projects, and the accounts with what they see.
#[derive(Serialize)]
struct WorldNow {
    here: MachineName,
    who: WorldWho,
    nodes: NodesReply,
    towns: Vec<TownSummary>,
    accounts: fastctx_types::accounts::AccountsReply,
}

/// One piece of a watched conversation, as the page receives it.
#[derive(Clone, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
enum Piece {
    #[cfg(target_os = "android")]
    Reset {
        history: Vec<SessionEvent>,
    },
    #[cfg(target_os = "android")]
    Interrupted {
        error: Option<String>,
    },
    Event {
        event: SessionEvent,
    },
    /// The watch is over: the node ended it, or the connection broke.
    Ended {
        error: Option<String>,
    },
}

#[derive(Serialize)]
struct Watch {
    id: u64,
    history: Vec<SessionEvent>,
}

#[derive(Serialize)]
struct Spoken {
    to: Address,
    landed: Landed,
}

fn resident(id: String) -> Result<ResidentId, String> {
    ResidentId::try_from(id).map_err(|error| error.to_string())
}

fn machine(name: Option<String>) -> Result<Option<MachineName>, String> {
    name.map(|name| name.parse::<MachineName>().map_err(|error| error.to_string())).transpose()
}

#[fastctx_gui_command::command]
async fn world_now(shell: tauri::State<'_, Shell>) -> Result<WorldNow, String> {
    let world = shell.world();
    let (who, nodes, accounts) = tokio::join!(world.who(None, true), world.nodes(), world.accounts());
    let nodes = nodes.map_err(|error| error.to_string())?;
    let who = who.map_err(|error| error.to_string())?;
    let accounts = accounts.map_err(|error| error.to_string())?;
    // Asked of the whole World, `who` lists its towns as well: the same
    // list `towns` would read again.
    let WhoScope::World { towns } = &who.scope else {
        return Err("the node answered about one town when asked about the World".to_string());
    };
    Ok(WorldNow { here: nodes.machine.clone(), towns: towns.clone(), who, nodes, accounts })
}

/// Starts watching a resident: returns what came before, and sends each
/// event after it down `pieces` until the page lets go.
#[fastctx_gui_command::command]
async fn watch(
    shell: tauri::State<'_, Shell>,
    resident_id: String,
    machine_name: Option<String>,
    pieces: Channel<Piece>,
) -> Result<Watch, String> {
    let page = shell.page.load(Ordering::Relaxed);
    let world = shell.world();
    let resident = resident(resident_id)?;
    let machine = machine(machine_name)?;
    #[cfg(not(target_os = "android"))]
    let (conversation, history) =
        Conversation::watch(&world, resident.clone(), machine.clone()).await.map_err(|error| error.to_string())?;
    #[cfg(target_os = "android")]
    let (conversation, history) = android::watching::open(&world, resident.clone(), machine.clone()).await?;
    let id = shell.next.fetch_add(1, Ordering::Relaxed);
    #[cfg(target_os = "android")]
    let work = android::watching::stream(world, resident, machine, conversation, pieces);
    #[cfg(not(target_os = "android"))]
    let work = async move {
        let mut conversation = conversation;
        loop {
            match conversation.next().await {
                Ok(Some(moment)) => {
                    if pieces.send(Piece::Event { event: moment.event }).is_err() {
                        return;
                    }
                }
                Ok(None) => {
                    let _ = pieces.send(Piece::Ended { error: None });
                    return;
                }
                Err(error) => {
                    let _ = pieces.send(Piece::Ended { error: Some(error.to_string()) });
                    return;
                }
            }
        }
    };
    shell.follow(page, id, work)?;
    Ok(Watch { id, history })
}

#[tauri::command]
fn unwatch(shell: tauri::State<'_, Shell>, id: u64) {
    if let Some(task) = shell.watches.lock().expect("watch table").remove(&id) {
        task.abort();
    }
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
) -> Result<Spoken, String> {
    let files_on = files_on
        .map(|written| written.parse::<fastctx_types::access::Written>().map_err(|error| error.to_string()))
        .transpose()?;
    let said = shell
        .world()
        .talk_with_attachments(
            resident(resident_id)?,
            machine(machine_name)?,
            text,
            attachments.unwrap_or_default(),
            files_on,
        )
        .await
        .map_err(|error| error.to_string())?;
    Ok(Spoken { to: said.to, landed: said.landed })
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

/// A journal's latest entries that fit, or those before `before`.
#[fastctx_gui_command::command]
async fn journal(
    shell: tauri::State<'_, Shell>,
    town: String,
    journal: String,
    before: Option<String>,
) -> Result<JournalView, String> {
    let town = TownName::try_from(town).map_err(|error| error.to_string())?;
    let journal = JournalName::try_from(journal).map_err(|error| error.to_string())?;
    let before = before.map(|id| EntryId::try_from(id).map_err(|error| error.to_string())).transpose()?;
    shell.world().journal(Some(town), journal, before).await.map_err(|error| error.to_string())
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
    let options = fastctx_types::agents::SessionOptions { model, reasoning_effort: effort };
    let (attachments, choices) = (attachments.unwrap_or_default(), choices.unwrap_or_default());
    let said = shell
        .world()
        .open_with_attachments(agent, town, machine(machine_name)?, text, options, attachments, choices)
        .await
        .map_err(|error| error.to_string())?;
    Ok(Spoken { to: said.to, landed: said.landed })
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

/// Removes a resident from the World, lets it sleep, wakes it or moves it
/// to another project, as the user; each stage is answered as it went.
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

/// A text file on a machine, or a piece of it, read for the user.
#[fastctx_gui_command::command]
async fn read_text(
    shell: tauri::State<'_, Shell>,
    machine_name: Option<String>,
    path: String,
    range: fastctx_types::file_read::FileRange,
) -> Result<reading::TextRead, String> {
    reading::read_text(&shell.world(), machine(machine_name)?, path, range).await
}

/// A file on a machine, whole, as bytes for the page to show as it is.
#[fastctx_gui_command::command]
async fn read_bytes(
    shell: tauri::State<'_, Shell>,
    machine_name: Option<String>,
    path: String,
) -> Result<reading::BytesRead, String> {
    reading::read_bytes(&shell.world(), machine(machine_name)?, path).await
}

/// Streams an original file into this device's cache for an external app.
#[fastctx_gui_command::command]
async fn fetch_file(
    app: tauri::AppHandle,
    shell: tauri::State<'_, Shell>,
    machine_name: Option<String>,
    path: String,
) -> Result<String, String> {
    let cache = app.path().app_cache_dir().map_err(|error| error.to_string())?;
    local_files::fetch(&shell.world(), machine(machine_name)?, path, &cache).await
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
) -> Result<attachment_files::Fetched, attachment_files::Failure> {
    let cache = app.path().app_cache_dir().map_err(|error| attachment_files::Failure::failed(error.to_string()))?;
    attachment_files::fetch(&shell.world(), attachment, &cache).await
}

#[fastctx_gui_command::command]
async fn open_local(app: tauri::AppHandle, path: String) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        android::open_file(&app, path).await
    }
    #[cfg(not(target_os = "android"))]
    {
        use tauri_plugin_opener::OpenerExt as _;
        app.opener().open_path(path, None::<&str>).map_err(|error| error.to_string())
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
) -> Result<tauri::ipc::Response, String> {
    binary_reading::read_binary(&shell.world(), machine(machine_name)?, path).await
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

#[fastctx_gui_command::command]
async fn journals(shell: tauri::State<'_, Shell>, name: String) -> Result<Vec<JournalSummary>, String> {
    Ok(shell.world().journals(Some(town(name)?)).await.map_err(|error| error.to_string())?.1)
}

/// An entry the user writes in a journal, starting the journal when it has
/// no description yet and one is given.
#[fastctx_gui_command::command]
async fn journal_add(
    shell: tauri::State<'_, Shell>,
    name: String,
    journal: String,
    text: String,
    description: Option<String>,
) -> Result<String, String> {
    let journal = JournalName::try_from(journal).map_err(|error| error.to_string())?;
    let reply = shell
        .world()
        .add_entry(Some(town(name)?), journal, text, description, Vec::new(), Vec::new(), None)
        .await
        .map_err(|error| error.to_string())?;
    match reply {
        fastctx_types::Reply::EntryAdded { id, .. } => Ok(id.to_string()),
        other => Err(format!("the node answered {other:?}")),
    }
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

#[cfg(desktop)]
#[fastctx_gui_command::command]
async fn renew_guardian_here(shell: tauri::State<'_, Shell>) -> Result<(), String> {
    shell.connection.local_program()?;
    tauri::async_runtime::spawn_blocking(renewal::guardian).await.map_err(|error| error.to_string())?
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
    #[cfg(all(windows, not(feature = "page-sandbox-probe")))]
    let Some(desktop) = desktop_presence::enter().expect("starting World's desktop presence") else {
        return;
    };
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_world_pages::init())
        // Its script also sends every link that opens a new window to the
        // system's browser, where the webview alone would drop it.
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let directory = app.path().app_data_dir()?;
            let connection = tauri::async_runtime::block_on(async { connection::Connection::open(directory) })
                .map_err(std::io::Error::other)?;
            app.manage(Shell {
                connection,
                watches: Arc::default(),
                next: AtomicU64::new(0),
                page: AtomicU64::new(0),
                line_requests: Default::default(),
            });
            #[cfg(desktop)]
            app.manage(removal::State::default());
            #[cfg(desktop)]
            app.manage(remote_world::Sessions::default());
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
            #[cfg(desktop)]
            removal::removal_plan,
            #[cfg(desktop)]
            removal::removal_run,
            #[cfg(desktop)]
            removal::removal_finish,
            #[cfg(desktop)]
            remote_world::remote_probe,
            #[cfg(desktop)]
            remote_world::remote_hub,
            #[cfg(desktop)]
            remote_world::remote_open,
            #[cfg(desktop)]
            remote_world::remote_join,
            #[cfg(desktop)]
            remote_world::remote_close,
            #[cfg(desktop)]
            retention::retention_read,
            #[cfg(desktop)]
            retention::retention_write,
            connection_state,
            select_connection,
            reconnect,
            accounts::login,
            accounts::login_token,
            accounts::open_account,
            accounts::accounts,
            accounts::machine_log,
            collab::collab_read,
            collab::collab_write,
            features::features_read,
            features::features_set,
            features::features_watch,
            accounts::account_change,
            accounts::server_command,
            accounts::agent_homes,
            accounts::works_on,
            world_now,
            since,
            agents,
            project,
            journal,
            journals,
            journal_add,
            project_new,
            project_edit,
            watch,
            unwatch,
            composer::composer_read,
            composer::composer_watch,
            composer::composer_update,
            composer::composer_send,
            composer::composer_attach_path,
            composer::composer_attach_bytes,
            talk,
            upload_path,
            upload_bytes,
            open,
            stop,
            stop_job,
            manage,
            claude_consent,
            find_files,
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
            open_local,
            save_as,
            reveal,
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
            renew_guardian_here,
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
    let builder = builder.plugin(desktop_presence::plugin(desktop));
    #[cfg(windows)]
    let builder = builder.plugin(titlebar::plugin());
    #[cfg(target_os = "android")]
    let builder = builder.plugin(android::plugin());
    let context = tauri::generate_context!();
    #[cfg(windows)]
    let context = titlebar::configure(context);
    builder.run(context).expect("running the World's window");
}
