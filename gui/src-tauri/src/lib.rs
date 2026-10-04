//! The shell between the web front end and the World. It holds nothing of
//! its own: every command is one call of the core's client
//! (`fastctx-client`), the same layer the TUI and the commands use, and a
//! watched conversation streams the session's events to the page as they
//! come. The page speaks as the person at this machine, never as a session.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use fastctx_client::{Conversation, World};
use fastctx_types::ipc::Peer;
use fastctx_types::layout::WorldDir;
use fastctx_types::project::{JournalView, MapView, TownSummary};
use fastctx_types::residents::Landed;
use fastctx_types::session::SessionEvent;
use fastctx_types::town::WorldWho;
use fastctx_types::world::NodesReply;
use fastctx_types::{Address, AgentName, EntryId, JournalName, MachineName, ResidentId, TownName};
use serde::Serialize;
use tauri::async_runtime::JoinHandle;
use tauri::ipc::Channel;

struct Shell {
    world: World,
    watches: Mutex<HashMap<u64, JoinHandle<()>>>,
    next: AtomicU64,
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
    let world = &shell.world;
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
    let (mut conversation, history) = Conversation::watch(&shell.world, resident(resident_id)?, machine(machine_name)?)
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

/// The user's words to a resident.
#[tauri::command]
async fn talk(
    shell: tauri::State<'_, Shell>,
    resident_id: String,
    machine_name: Option<String>,
    text: String,
) -> Result<Spoken, String> {
    let said = shell
        .world
        .talk(resident(resident_id)?, machine(machine_name)?, text)
        .await
        .map_err(|error| error.to_string())?;
    Ok(Spoken { to: said.to, landed: said.landed })
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
    let (map, who) = tokio::join!(shell.world.map(Some(town.clone())), shell.world.who(Some(town), false));
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
    shell.world.journal(Some(town), journal, before).await.map_err(|error| error.to_string())
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
) -> Result<Spoken, String> {
    let agent = AgentName::try_from(agent.as_str()).map_err(|error| error.to_string())?;
    let town = town.map(|town| TownName::try_from(town).map_err(|error| error.to_string())).transpose()?;
    let said = shell.world.open(agent, town, machine(machine_name)?, text).await.map_err(|error| error.to_string())?;
    Ok(Spoken { to: said.to, landed: said.landed })
}

/// Stops the turn a resident is running; says whether one was.
#[tauri::command]
async fn stop(
    shell: tauri::State<'_, Shell>,
    resident_id: String,
    machine_name: Option<String>,
) -> Result<bool, String> {
    let (_, stopped) =
        shell.world.stop(resident(resident_id)?, machine(machine_name)?).await.map_err(|error| error.to_string())?;
    Ok(stopped)
}

pub fn run() {
    let root = fastctx_platform::fastctx_root().expect("finding this user's fastctx directory");
    // When no node runs, it is started from the stable copy fastctx keeps
    // for the hosts, never from this program.
    let program = root.join("bin").join(if cfg!(windows) { "fastctx.exe" } else { "fastctx" });
    let world =
        World::new(WorldDir::under(&root), Peer::Command { resident: None, session: None }).starting_node_from(program);
    tauri::Builder::default()
        .manage(Shell { world, watches: Mutex::new(HashMap::new()), next: AtomicU64::new(0) })
        .invoke_handler(tauri::generate_handler![world_now, project, journal, watch, unwatch, talk, open, stop])
        .run(tauri::generate_context!())
        .expect("running the World's window");
}
