//! The shell exposes shared input without keeping a draft or releasing a queue itself.

use fastctx_client::{World, composer::Composing};
use fastctx_types::attachments::Attachment;
use fastctx_types::composer::{Change, Composer, Mutation, Request, Target, Updated};
use fastctx_types::world::LinkState;
use fastctx_types::{MachineName, ResidentId};
use serde::Serialize;
use tauri::ipc::Channel;

use crate::{Shell, machine, resident};

#[derive(Serialize)]
pub struct Watching {
    id: u64,
    watch: String,
    composer: Composer,
}

#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Piece {
    Snapshot { watch: String, composer: Composer },
    Interrupted { error: Option<String> },
    Ended { error: Option<String> },
}

#[tauri::command]
pub async fn composer_read(
    shell: tauri::State<'_, Shell>,
    resident_id: String,
    machine_name: Option<String>,
) -> Result<Updated, String> {
    shell
        .world()
        .composer(resident(resident_id)?, machine(machine_name)?, Request::Read)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn composer_update(
    shell: tauri::State<'_, Shell>,
    resident_id: String,
    machine_name: Option<String>,
    mutation: Mutation,
) -> Result<Updated, String> {
    shell
        .world()
        .update_composer(resident(resident_id)?, machine(machine_name)?, mutation)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn composer_send(
    shell: tauri::State<'_, Shell>,
    resident_id: String,
    machine_name: Option<String>,
    operation_id: String,
    epoch: u64,
) -> Result<Updated, String> {
    composer_update(
        shell,
        resident_id,
        machine_name,
        Mutation { operation: operation_id, change: Change::Send { epoch } },
    )
    .await
}

#[tauri::command]
pub async fn composer_watch(
    shell: tauri::State<'_, Shell>,
    resident_id: String,
    machine_name: Option<String>,
    pieces: Channel<Piece>,
) -> Result<Watching, String> {
    let world = shell.world();
    let resident = resident(resident_id)?;
    let machine = machine(machine_name)?;
    let (composing, watch, composer) = open(&world, &resident, &machine).await?;
    let id = shell.next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let task = tauri::async_runtime::spawn(stream(world, resident, machine, composing, watch.clone(), pieces));
    shell.watches.lock().expect("watch table").insert(id, task);
    Ok(Watching { id, watch, composer })
}

async fn open(
    world: &World,
    resident: &ResidentId,
    machine: &Option<MachineName>,
) -> Result<(Composing, String, Composer), String> {
    let mut changes =
        world.device().map(|device| device.connection_changes()).transpose().map_err(|error| error.to_string())?;
    loop {
        if let Some(device) = world.device() {
            let connection = changes.as_mut().expect("device changes");
            loop {
                connection.borrow_and_update();
                match device.state().link {
                    LinkState::Online { .. } => break,
                    LinkState::Stopped { reason } => return Err(reason),
                    LinkState::Alone => return Err("this device has not joined a World".into()),
                    _ => connection.changed().await.map_err(|error| error.to_string())?,
                }
            }
        }
        #[cfg(target_os = "android")]
        crate::android::readiness::wait(world).await?;
        let name = fastctx_client::composer::watch_id().map_err(|error| error.to_string())?;
        let read = tokio::select! {
            changed = async { match changes.as_mut() { Some(changes) => changes.changed().await.map_err(|error| error.to_string()), None => std::future::pending().await } } => {
                changed?;
                continue;
            },
            read = world.watch_composer(resident.clone(), machine.clone(), name.clone()) => read,
        };
        let (composing, composer) = read.map_err(|error| error.to_string())?;
        return Ok((composing, name, composer));
    }
}

async fn stream(
    world: World,
    resident: ResidentId,
    machine: Option<MachineName>,
    initial: Composing,
    name: String,
    pieces: Channel<Piece>,
) {
    let mut changes = match world.device() {
        Some(device) => match device.connection_changes() {
            Ok(changes) => Some(changes),
            Err(error) => {
                let _ = pieces.send(Piece::Ended { error: Some(error.to_string()) });
                return;
            }
        },
        None => None,
    };
    #[cfg(target_os = "android")]
    let mut visible = crate::android::visibility();
    let mut composing = Some(initial);
    let mut name = name;
    loop {
        #[cfg(target_os = "android")]
        if !*visible.borrow_and_update() {
            composing = None;
        }
        let next = tokio::select! {
            changed = async { match changes.as_mut() { Some(changes) => changes.changed().await.map_err(|error| error.to_string()), None => std::future::pending().await } } => {
                if let Err(error) = changed { let _ = pieces.send(Piece::Ended { error: Some(error) }); return; }
                None
            },
            _ = async {
                #[cfg(target_os = "android")]
                { visible.changed().await }
                #[cfg(not(target_os = "android"))]
                { std::future::pending::<Result<(), tokio::sync::watch::error::RecvError>>().await }
            } => None,
            next = async { match composing.as_mut() { Some(composing) => composing.next().await, None => std::future::pending().await } } => Some(next),
        };
        match next {
            Some(Ok(Some(composer))) => {
                if pieces.send(Piece::Snapshot { watch: name.clone(), composer }).is_err() {
                    return;
                }
                continue;
            }
            Some(ended) => {
                drop(composing.take());
                let error = ended.err().map(|error| error.to_string());
                if changes.is_none() {
                    let _ = pieces.send(Piece::Ended { error });
                    return;
                }
                if pieces.send(Piece::Interrupted { error }).is_err() {
                    return;
                }
                tokio::select! {
                    changed = changes.as_mut().expect("device changes").changed() => if changed.is_err() { return; },
                    _ = async {
                        #[cfg(target_os = "android")]
                        { visible.changed().await }
                        #[cfg(not(target_os = "android"))]
                        { std::future::pending::<Result<(), tokio::sync::watch::error::RecvError>>().await }
                    } => {},
                }
            }
            None => {
                drop(composing.take());
            }
        }
        #[cfg(target_os = "android")]
        if !*visible.borrow_and_update() {
            continue;
        }
        match open(&world, &resident, &machine).await {
            Ok((fresh, watch, composer)) => {
                name = watch;
                if pieces.send(Piece::Snapshot { watch: name.clone(), composer }).is_err() {
                    return;
                }
                composing = Some(fresh);
            }
            Err(error) => {
                let _ = pieces.send(Piece::Ended { error: Some(error) });
                return;
            }
        }
    }
}

/// A completed upload receipt is returned even when the draft changed during transfer.
#[derive(Serialize)]
pub struct AttachmentAdded {
    attachment: Attachment,
    updated: Result<Updated, String>,
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn composer_attach_path(
    shell: tauri::State<'_, Shell>,
    resident_id: String,
    machine_name: Option<String>,
    operation_id: String,
    target: Target,
    epoch: u64,
    path: String,
    sent: Channel<u64>,
) -> Result<AttachmentAdded, String> {
    let world = shell.world();
    let resident = resident(resident_id)?;
    let machine = machine(machine_name)?;
    #[cfg(target_os = "android")]
    crate::android::readiness::wait(&world).await?;
    let attachment = crate::uploading::composer_path(
        &world,
        resident,
        machine,
        fastctx_types::composer::UploadScope { operation: operation_id, target, epoch },
        std::path::Path::new(&path),
        sent,
    )
    .await?;
    Ok(AttachmentAdded {
        attachment: attachment.attachment,
        updated: attachment.updated.map_err(|error| error.message),
    })
}

#[tauri::command]
pub async fn composer_attach_bytes(
    shell: tauri::State<'_, Shell>,
    request: tauri::ipc::Request<'_>,
) -> Result<AttachmentAdded, String> {
    let header = |name: &str| -> Result<String, String> {
        crate::decoded(
            request
                .headers()
                .get(name)
                .ok_or(format!("the upload names no {name}"))?
                .to_str()
                .map_err(|error| error.to_string())?,
        )
    };
    let resident = resident(header("resident")?)?;
    let machine = machine(Some(header("machine")?).filter(|name| !name.is_empty()))?;
    let operation = header("operation")?;
    let target = serde_json::from_str(&header("target")?).map_err(|error| format!("upload target: {error}"))?;
    let epoch = header("epoch")?.parse().map_err(|error| format!("upload epoch: {error}"))?;
    #[cfg(target_os = "android")]
    let bytes = crate::android::uploading::bytes(request.body())?;
    #[cfg(not(target_os = "android"))]
    let bytes = match request.body() {
        tauri::ipc::InvokeBody::Raw(bytes) => bytes.clone(),
        _ => return Err("the attachment did not come as bytes".into()),
    };
    let world = shell.world();
    #[cfg(target_os = "android")]
    crate::android::readiness::wait(&world).await?;
    let attachment = crate::uploading::composer_bytes(
        &world,
        resident,
        machine,
        fastctx_types::composer::UploadScope { operation, target, epoch },
        header("name")?,
        header("type")?,
        bytes,
    )
    .await?;
    Ok(AttachmentAdded {
        attachment: attachment.attachment,
        updated: attachment.updated.map_err(|error| error.message),
    })
}
