//! Line-source command contracts, growth channels and explicit cancellation.

use crate::Shell;
use fastctx_types::access::Written;

use fastctx_types::ipc::Failure;
use fastctx_types::lines::{Decoding, Direction, Extent, Match, Position, Request, Result as Answer};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, atomic::Ordering};
use tauri::ipc::Channel;

#[derive(Default)]
pub(crate) struct Requests(Arc<Mutex<HashMap<String, tokio::task::AbortHandle>>>);
impl Requests {
    pub(crate) fn cancel_all(&self) {
        for (_, task) in self.0.lock().expect("line requests").drain() {
            task.abort();
        }
    }
}
struct Pending {
    requests: Arc<Mutex<HashMap<String, tokio::task::AbortHandle>>>,
    id: String,
    task: tokio::task::Id,
}
impl Drop for Pending {
    fn drop(&mut self) {
        let mut requests = self.requests.lock().expect("line requests");
        if requests.get(&self.id).is_some_and(|handle| handle.id() == self.task)
            && let Some(task) = requests.remove(&self.id)
        {
            task.abort();
        }
    }
}

#[derive(Serialize)]
pub(crate) struct Opened {
    machine: Written,
    source: String,
    path: String,
    extent: Extent,
    watch: u64,
}
#[derive(Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub(crate) enum Change {
    Changed { extent: Extent, first_changed_line: u64 },
    Error { message: String },
}

#[fastctx_gui_command::command]
pub(crate) async fn lines_open(
    shell: tauri::State<'_, Shell>,
    machine_name: Option<String>,
    path: String,
    decoding: Decoding,
    channel: Channel<Change>,
) -> Result<Opened, Failure> {
    let page = shell.page.load(Ordering::Relaxed);
    let world = shell.world();
    let machine = crate::machine(machine_name).map_err(Failure::new)?;
    let reply = world.open_lines(machine.clone(), path, decoding).await.map_err(open_failed)?;
    let Answer::Opened { source, path, extent } = reply.result else {
        return Err(Failure::new("expected an opened line source"));
    };
    let machine = machine.unwrap_or_else(|| reply.machine.clone().into());
    let watch = shell.next.fetch_add(1, Ordering::Relaxed);
    let (held_source, held_machine, initial_version) = (source.clone(), machine.clone(), extent.version);
    let watching_world = world.clone();
    let followed = shell.follow(page, watch, async move {
        let world = watching_world;
        let mut version = initial_version;
        loop {
            let result =
                world.lines(held_machine.clone(), Request::Changed { source: held_source.clone(), version }).await;
            match result {
                Ok(reply) => match reply.result {
                    Answer::Changed { extent, first_changed_line } => {
                        version = extent.version;
                        if channel.send(Change::Changed { extent, first_changed_line }).is_err() {
                            break;
                        }
                    }
                    _ => {
                        let _ = channel.send(Change::Error { message: "expected a line-source change".into() });
                        break;
                    }
                },
                Err(error) => {
                    let _ = channel.send(Change::Error { message: error.to_string() });
                    break;
                }
            }
        }
        let _ = world.lines(held_machine, Request::Close { source: held_source }).await;
    });
    if let Err(error) = followed {
        world
            .lines(machine.clone(), Request::Close { source: source.clone() })
            .await
            .map_err(|closing| Failure::new(format!("{error}; closing its line source: {closing}")))?;
        return Err(Failure::new(error));
    }
    Ok(Opened { machine, source, path, extent, watch })
}

fn open_failed(error: fastctx_client::Error) -> Failure {
    match error {
        fastctx_client::Error::Call(fastctx_client::CallError::Failed(failure)) => failure,
        error => Failure::new(error.to_string()),
    }
}

async fn request(shell: &Shell, machine_name: String, request_id: String, request: Request) -> Result<Answer, String> {
    let machine = Written::parse(&machine_name).map_err(|error| error.to_string())?;
    let world = shell.world();
    let task = {
        let mut requests = shell.line_requests.0.lock().expect("line requests");
        if requests.contains_key(&request_id) {
            return Err("line request id is already in use".into());
        }
        let task = tokio::spawn(async move { world.lines(machine, request).await });
        requests.insert(request_id.clone(), task.abort_handle());
        task
    };
    let _pending = Pending { requests: shell.line_requests.0.clone(), id: request_id, task: task.id() };
    task.await.map_err(|error| error.to_string())?.map(|reply| reply.result).map_err(|error| error.to_string())
}

#[fastctx_gui_command::command]
pub(crate) async fn lines_read(
    shell: tauri::State<'_, Shell>,
    machine_name: String,
    source: String,
    start: u64,
    count: u64,
    request_id: String,
) -> Result<Vec<String>, String> {
    match request(&shell, machine_name, request_id, Request::Read { source, start, count }).await? {
        Answer::Lines { lines, .. } => Ok(lines),
        _ => Err("expected logical lines".into()),
    }
}

#[fastctx_gui_command::command]
pub(crate) async fn lines_read_projected(
    shell: tauri::State<'_, Shell>,
    machine_name: String,
    source: String,
    start: u64,
    count: u64,
    request_id: String,
) -> Result<fastctx_types::lines::ProjectedLines, String> {
    match request(&shell, machine_name, request_id, Request::Read { source, start, count }).await? {
        Answer::Lines { start, lines, notes } => Ok(fastctx_types::lines::ProjectedLines { start, lines, notes }),
        _ => Err("expected logical lines".into()),
    }
}

#[fastctx_gui_command::command]
pub(crate) async fn lines_find(
    shell: tauri::State<'_, Shell>,
    machine_name: String,
    source: String,
    query: String,
    position: Position,
    direction: Direction,
    request_id: String,
) -> Result<Option<Match>, String> {
    match request(&shell, machine_name, request_id, Request::Find { source, query, position, direction }).await? {
        Answer::Found { found } => Ok(found),
        _ => Err("expected a literal match".into()),
    }
}

#[fastctx_gui_command::command]
pub(crate) async fn lines_close(
    shell: tauri::State<'_, Shell>,
    machine_name: String,
    source: String,
    watch: u64,
) -> Result<(), String> {
    if let Some(task) = shell.watches.lock().expect("watch table").remove(&watch) {
        task.abort();
    }
    let machine = Written::parse(&machine_name).map_err(|error| error.to_string())?;
    shell.world().lines(machine, Request::Close { source }).await.map(|_| ()).map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) fn cancel_line_request(shell: tauri::State<'_, Shell>, request_id: String) {
    if let Some(task) = shell.line_requests.0.lock().expect("line requests").remove(&request_id) {
        task.abort();
    }
}
