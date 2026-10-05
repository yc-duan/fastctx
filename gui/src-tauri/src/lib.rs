//! The shell between the web front end and the World. It holds nothing of
//! its own: every command is one call of the core's client
//! (`fastctx-client`), the same layer the TUI and the commands use, and a
//! watched conversation streams the session's events to the page as they
//! come. The page speaks as the person at this machine, never as a session.

mod connection;

#[cfg(desktop)]
mod setup;
#[cfg(desktop)]
mod terminal;
#[cfg(desktop)]
mod user_path;

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use fastctx_client::{Conversation, World};
use fastctx_types::device::DeviceJoined;
use fastctx_types::project::{JournalSummary, JournalView, MapView, Record, RecordFields, Stamp, TownSummary};
use fastctx_types::residents::Landed;
use fastctx_types::session::SessionEvent;
use fastctx_types::session::control::{Control, Controlled, Offers};
use fastctx_types::town::{LookedUp, WorldWho};
use fastctx_types::world::NodesReply;
use fastctx_types::{Address, AgentName, EntryId, JournalName, MachineName, ResidentId, TownName};
use serde::Serialize;
use tauri::Manager as _;
use tauri::async_runtime::JoinHandle;
use tauri::ipc::Channel;

#[cfg(windows)]
mod desktop_presence;
mod reading;
mod uploading;

struct Shell {
    connection: connection::Connection,
    watches: Mutex<HashMap<u64, JoinHandle<()>>>,
    next: AtomicU64,
}

impl Shell {
    fn world(&self) -> World {
        self.connection.world()
    }
}

/// Available before joining and while no machines can answer.
#[tauri::command]
fn connection_state(shell: tauri::State<'_, Shell>) -> connection::State {
    shell.connection.state()
}

/// Desktop can use a local node or a device identity; mobile uses a device.
#[tauri::command]
async fn select_connection(
    shell: tauri::State<'_, Shell>,
    mode: connection::Mode,
) -> Result<connection::State, String> {
    let (state, changed) = shell.connection.select(mode)?;
    if changed {
        for (_, watch) in shell.watches.lock().expect("watch table").drain() {
            watch.abort();
        }
    }
    Ok(state)
}

/// A scanned code and a pasted code take the same path. No service is installed.
#[tauri::command]
async fn join_device(
    shell: tauri::State<'_, Shell>,
    code: String,
    name: Option<String>,
) -> Result<DeviceJoined, String> {
    let world = shell.world();
    if world.device().is_none() {
        return Err("select the device connection before joining with a device invitation".into());
    }
    let (world, name) = world.join(code, machine(name)?).await.map_err(|error| error.to_string())?;
    Ok(DeviceJoined { world, name })
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

/// The World as the front page reads it, asked in one go.
#[derive(Serialize)]
struct WorldNow {
    here: MachineName,
    who: WorldWho,
    nodes: NodesReply,
    towns: Vec<TownSummary>,
}

/// One piece of a watched conversation, as the page receives it.
#[derive(Clone, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
enum Piece {
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

#[tauri::command]
async fn world_now(shell: tauri::State<'_, Shell>) -> Result<WorldNow, String> {
    let world = shell.world();
    let (who, nodes, towns) = tokio::join!(world.who(None, true), world.nodes(), world.towns());
    let nodes = nodes.map_err(|error| error.to_string())?;
    Ok(WorldNow {
        here: nodes.machine.clone(),
        who: who.map_err(|error| error.to_string())?,
        nodes,
        towns: towns.map_err(|error| error.to_string())?,
    })
}

/// Starts watching a resident: returns what came before, and sends each
/// event after it down `pieces` until the page lets go.
#[tauri::command]
async fn watch(
    shell: tauri::State<'_, Shell>,
    resident_id: String,
    machine_name: Option<String>,
    pieces: Channel<Piece>,
) -> Result<Watch, String> {
    let (mut conversation, history) =
        Conversation::watch(&shell.world(), resident(resident_id)?, machine(machine_name)?)
            .await
            .map_err(|error| error.to_string())?;
    let id = shell.next.fetch_add(1, Ordering::Relaxed);
    let task = tauri::async_runtime::spawn(async move {
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
    });
    shell.watches.lock().expect("watch table").insert(id, task);
    Ok(Watch { id, history })
}

#[tauri::command]
fn unwatch(shell: tauri::State<'_, Shell>, id: u64) {
    if let Some(task) = shell.watches.lock().expect("watch table").remove(&id) {
        task.abort();
    }
}

/// The user's words to a resident, with the files uploaded for them.
#[tauri::command]
async fn talk(
    shell: tauri::State<'_, Shell>,
    resident_id: String,
    machine_name: Option<String>,
    text: String,
    attachments: Option<Vec<fastctx_types::attachments::Attachment>>,
) -> Result<Spoken, String> {
    let said = shell
        .world()
        .talk_with_attachments(resident(resident_id)?, machine(machine_name)?, text, attachments.unwrap_or_default())
        .await
        .map_err(|error| error.to_string())?;
    Ok(Spoken { to: said.to, landed: said.landed })
}

/// Carries a file on this computer to a resident's machine, for words to
/// it; `sent` hears how much has gone.
#[tauri::command]
async fn upload_path(
    shell: tauri::State<'_, Shell>,
    resident_id: String,
    machine_name: Option<String>,
    path: String,
    sent: Channel<u64>,
) -> Result<fastctx_types::attachments::Attachment, String> {
    let target = fastctx_types::attachments::UploadTarget::Resident { resident: resident(resident_id)? };
    uploading::from_path(&shell.world(), target, machine(machine_name)?, std::path::Path::new(&path), sent).await
}

/// Carries bytes the page holds to a resident's machine. The bytes are the
/// body; whom they are for and what they are come as headers, each
/// percent-encoded, since a name may be in any language.
#[tauri::command]
async fn upload_bytes(
    shell: tauri::State<'_, Shell>,
    request: tauri::ipc::Request<'_>,
) -> Result<fastctx_types::attachments::Attachment, String> {
    let header = |name: &str| -> Result<String, String> {
        let raw = request.headers().get(name).ok_or(format!("the upload names no {name}"))?;
        decoded(raw.to_str().map_err(|error| error.to_string())?)
    };
    let tauri::ipc::InvokeBody::Raw(bytes) = request.body() else {
        return Err("the attachment's bytes did not come as bytes".into());
    };
    let target = fastctx_types::attachments::UploadTarget::Resident { resident: resident(header("resident")?)? };
    let machine = machine(Some(header("machine")?).filter(|name| !name.is_empty()))?;
    uploading::from_bytes(&shell.world(), target, machine, header("name")?, header("type")?, bytes.clone()).await
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

#[tauri::command]
async fn project(shell: tauri::State<'_, Shell>, town: String) -> Result<Project, String> {
    let town = TownName::try_from(town).map_err(|error| error.to_string())?;
    let world = shell.world();
    let (map, who) = tokio::join!(world.map(Some(town.clone())), world.who(Some(town), false));
    Ok(Project { map: map.map_err(|error| error.to_string())?, who: who.map_err(|error| error.to_string())? })
}

/// A journal's latest entries that fit, or those before `before`.
#[tauri::command]
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
#[tauri::command]
async fn open(
    shell: tauri::State<'_, Shell>,
    agent: String,
    town: Option<String>,
    machine_name: Option<String>,
    text: String,
    model: Option<String>,
    effort: Option<String>,
) -> Result<Spoken, String> {
    let agent = AgentName::try_from(agent.as_str()).map_err(|error| error.to_string())?;
    let town = town.map(|town| TownName::try_from(town).map_err(|error| error.to_string())).transpose()?;
    let options = fastctx_types::agents::SessionOptions { model, reasoning_effort: effort };
    let said = shell
        .world()
        .open_with_options(agent, town, machine(machine_name)?, text, options)
        .await
        .map_err(|error| error.to_string())?;
    Ok(Spoken { to: said.to, landed: said.landed })
}

/// What happened in the World after a moment, as each machine's records
/// say now; the whole World, or one project's machines.
#[tauri::command]
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
#[tauri::command]
async fn agents(
    shell: tauri::State<'_, Shell>,
    machine_name: Option<String>,
) -> Result<fastctx_types::agents::WorldAgents, String> {
    shell.world().agents(machine(machine_name)?).await.map_err(|error| error.to_string())
}

/// Stops the turn a resident is running; says whether one was.
#[tauri::command]
async fn stop(
    shell: tauri::State<'_, Shell>,
    resident_id: String,
    machine_name: Option<String>,
) -> Result<bool, String> {
    let (_, stopped) =
        shell.world().stop(resident(resident_id)?, machine(machine_name)?).await.map_err(|error| error.to_string())?;
    Ok(stopped)
}

/// Does something to a resident's conversation for the user, besides
/// speaking into it.
#[tauri::command]
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
#[tauri::command]
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
#[tauri::command]
async fn read_text(
    shell: tauri::State<'_, Shell>,
    machine_name: Option<String>,
    path: String,
    range: fastctx_types::file_read::FileRange,
) -> Result<reading::TextRead, String> {
    reading::read_text(&shell.world(), machine(machine_name)?, path, range).await
}

/// A file on a machine, whole, as bytes for the page to show as it is.
#[tauri::command]
async fn read_bytes(
    shell: tauri::State<'_, Shell>,
    machine_name: Option<String>,
    path: String,
) -> Result<reading::BytesRead, String> {
    reading::read_bytes(&shell.world(), machine(machine_name)?, path).await
}

/// Residents or jobs in detail, by id, from every machine that has them.
#[tauri::command]
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

/// Opens a project with the first version of its record.
#[tauri::command]
async fn project_new(shell: tauri::State<'_, Shell>, name: String, fields: RecordFields) -> Result<TownRecord, String> {
    let made = shell.world().open_town(town(name)?, fields).await.map_err(|error| error.to_string())?;
    Ok(TownRecord { town: made.town, record: made.record, opened: made.opened })
}

/// Writes the next version of a project's record; what is not given stays.
#[tauri::command]
async fn project_edit(
    shell: tauri::State<'_, Shell>,
    name: String,
    fields: RecordFields,
) -> Result<TownRecord, String> {
    let made = shell.world().edit_town(Some(town(name)?), fields).await.map_err(|error| error.to_string())?;
    Ok(TownRecord { town: made.town, record: made.record, opened: made.opened })
}

#[tauri::command]
async fn journals(shell: tauri::State<'_, Shell>, name: String) -> Result<Vec<JournalSummary>, String> {
    Ok(shell.world().journals(Some(town(name)?)).await.map_err(|error| error.to_string())?.1)
}

/// An entry the user writes in a journal, starting the journal when it has
/// no description yet and one is given.
#[tauri::command]
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

/// A one-use code that lets one more machine join, valid for `hours`.
#[tauri::command]
async fn invite(
    shell: tauri::State<'_, Shell>,
    name: Option<String>,
    hours: u32,
    device: Option<bool>,
) -> Result<String, String> {
    let expires = jiff::Timestamp::now()
        .checked_add(jiff::SignedDuration::from_hours(i64::from(hours)))
        .map_err(|error| error.to_string())?;
    shell.world().invite(machine(name)?, expires, device.unwrap_or(false)).await.map_err(|error| error.to_string())
}

/// No machine trusts `name`'s key any more.
#[tauri::command]
async fn revoke(shell: tauri::State<'_, Shell>, name: String, reason: String) -> Result<(), String> {
    let name = name.parse::<MachineName>().map_err(|error| error.to_string())?;
    shell.world().revoke(name, reason).await.map_err(|error| error.to_string())
}

/// This machine leaves its World and becomes a World of one again.
#[tauri::command]
async fn leave(shell: tauri::State<'_, Shell>) -> Result<(), String> {
    shell.world().leave().await.map_err(|error| error.to_string())
}

/// Opens a resident on this computer in its own official interface, in a
/// terminal window of its own; resolves when that window's attach ends.
#[cfg(desktop)]
#[tauri::command]
async fn attach_here(shell: tauri::State<'_, Shell>, resident_id: String) -> Result<(), String> {
    let program = shell.connection.local_program()?;
    let resident = resident(resident_id)?;
    tauri::async_runtime::spawn_blocking(move || terminal::attach(&program, &resident))
        .await
        .map_err(|error| error.to_string())?
}

/// Where this computer stands with the World.
#[cfg(desktop)]
#[tauri::command]
async fn setup_state(shell: tauri::State<'_, Shell>) -> Result<setup::Setup, String> {
    shell.connection.local_program()?;
    tauri::async_runtime::spawn_blocking(setup::setup).await.map_err(|error| error.to_string())?
}

/// Puts the fastctx this application carries in place for the hosts.
#[cfg(desktop)]
#[tauri::command]
async fn place_fastctx(shell: tauri::State<'_, Shell>) -> Result<String, String> {
    shell.connection.local_program()?;
    tauri::async_runtime::spawn_blocking(setup::place).await.map_err(|error| error.to_string())?
}

/// Joins a World with a code, under `name` or the name this machine has.
#[cfg(desktop)]
#[tauri::command]
async fn join_world(
    shell: tauri::State<'_, Shell>,
    code: String,
    name: Option<String>,
) -> Result<setup::Joined, String> {
    let world = shell.connection.local_world()?;
    setup::join(&world, code.trim().to_string(), machine(name)?).await
}

/// Has a system service keep this computer's node, asking once for
/// administrator rights.
#[cfg(desktop)]
#[tauri::command]
async fn keep_here(shell: tauri::State<'_, Shell>) -> Result<setup::KeptNow, String> {
    // A computer that joined as a device has no node to keep.
    shell.connection.local_program()?;
    setup::keep(&shell.world()).await
}

/// Removes the service that keeps this computer's node.
#[cfg(desktop)]
#[tauri::command]
async fn release_here() -> Result<setup::KeptNow, String> {
    tauri::async_runtime::spawn_blocking(setup::release).await.map_err(|error| error.to_string())?
}

/// Founds a hub on this computer; returns the code that founds its World.
#[cfg(desktop)]
#[tauri::command]
async fn hub_here(shell: tauri::State<'_, Shell>, public: Vec<String>) -> Result<String, String> {
    shell.connection.local_program()?;
    tauri::async_runtime::spawn_blocking(move || setup::hub_here(public)).await.map_err(|error| error.to_string())?
}

/// Puts the World's tools into this computer's hosts, or takes them out.
#[cfg(desktop)]
#[tauri::command]
async fn apply_hosts(
    shell: tauri::State<'_, Shell>,
    codex: bool,
    claude: bool,
    raise: bool,
    into: bool,
) -> Result<Vec<String>, String> {
    shell.connection.local_program()?;
    tauri::async_runtime::spawn_blocking(move || setup::hosts(codex, claude, raise, into))
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
    #[cfg(windows)]
    let Some(desktop) = desktop_presence::enter().expect("starting World's desktop presence") else {
        return;
    };
    let builder = tauri::Builder::default()
        // Its script also sends every link that opens a new window to the
        // system's browser, where the webview alone would drop it.
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let directory = app.path().app_data_dir()?;
            let connection = tauri::async_runtime::block_on(async { connection::Connection::open(directory) })
                .map_err(std::io::Error::other)?;
            app.manage(Shell { connection, watches: Mutex::new(HashMap::new()), next: AtomicU64::new(0) });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            connection_state,
            select_connection,
            join_device,
            reconnect,
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
            talk,
            upload_path,
            upload_bytes,
            open,
            stop,
            look,
            read_text,
            read_bytes,
            control,
            offers,
            invite,
            revoke,
            leave,
            #[cfg(desktop)]
            setup_state,
            #[cfg(desktop)]
            place_fastctx,
            #[cfg(desktop)]
            join_world,
            #[cfg(desktop)]
            hub_here,
            #[cfg(desktop)]
            apply_hosts,
            #[cfg(desktop)]
            attach_here,
            #[cfg(desktop)]
            keep_here,
            #[cfg(desktop)]
            release_here
        ]);
    #[cfg(desktop)]
    let builder = builder.plugin(tauri_plugin_window_state::Builder::default().with_state_flags(WINDOW_KEPT).build());
    #[cfg(windows)]
    let builder = builder.plugin(desktop_presence::plugin(desktop));
    builder.run(tauri::generate_context!()).expect("running the World's window");
}
