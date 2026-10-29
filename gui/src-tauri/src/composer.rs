//! The shell exposes shared input without keeping a draft or releasing a queue itself.

use fastctx_client::{World, composer::Composing};
use fastctx_net::link::Status;
use fastctx_types::ResidentId;
use fastctx_types::access::Written;
use fastctx_types::attachments::Attachment;
use fastctx_types::composer::{Change, Composer, Mutation, Request, Target, Updated};
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
    Snapshot {
        watch: String,
        composer: Box<Composer>,
    },
    /// The watch broke and is being opened again: why, and since when
    /// (milliseconds since the Unix epoch).
    Interrupted {
        error: Option<String>,
        since: u64,
    },
}

/// A request the page sent that came to nothing: `again` when no answer
/// came, so it is sent again, as it was, once the shared input is back.
fn not_done(error: fastctx_client::Error) -> crate::NotSaid {
    crate::NotSaid { again: World::unanswered(&error), message: error.to_string() }
}

fn refused(message: String) -> crate::NotSaid {
    crate::NotSaid { message, again: false }
}

#[fastctx_gui_command::command]
pub async fn composer_read(
    shell: tauri::State<'_, Shell>,
    resident_id: String,
    machine_name: Option<String>,
) -> Result<Updated, crate::NotSaid> {
    shell
        .world()
        .composer(resident(resident_id).map_err(refused)?, machine(machine_name).map_err(refused)?, Request::Read)
        .await
        .map_err(not_done)
}

#[fastctx_gui_command::command]
pub async fn composer_update(
    shell: tauri::State<'_, Shell>,
    resident_id: String,
    machine_name: Option<String>,
    mutation: Mutation,
) -> Result<Updated, crate::NotSaid> {
    shell
        .world()
        .update_composer(resident(resident_id).map_err(refused)?, machine(machine_name).map_err(refused)?, mutation)
        .await
        .map_err(not_done)
}

#[fastctx_gui_command::command]
pub async fn composer_send(
    shell: tauri::State<'_, Shell>,
    resident_id: String,
    machine_name: Option<String>,
    operation_id: String,
    epoch: u64,
) -> Result<Updated, crate::NotSaid> {
    composer_update(
        shell,
        resident_id,
        machine_name,
        Mutation { operation: operation_id, change: Change::Send { epoch } },
    )
    .await
}

#[fastctx_gui_command::command]
pub async fn composer_watch(
    shell: tauri::State<'_, Shell>,
    resident_id: String,
    machine_name: Option<String>,
    pieces: Channel<crate::watch_delivery::Wake>,
) -> Result<Watching, String> {
    let page = shell.page.load(std::sync::atomic::Ordering::Relaxed);
    let world = shell.world();
    let resident = resident(resident_id)?;
    let machine = machine(machine_name)?;
    let (composing, watch, composer) = open(&world, &resident, &machine).await.map_err(|not| not.why)?;
    let id = shell.next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut initial = Some((composing, watch.clone()));
    shell.follow_delivery(
        page,
        id,
        pieces,
        |_: &Piece| Some("composer".into()),
        move |pieces, _| {
            let (world, resident, machine) = (world.clone(), resident.clone(), machine.clone());
            let initial = initial.take();
            async move {
                let (composing, name) = if let Some(initial) = initial {
                    initial
                } else {
                    let mut again = fastctx_client::Again::default();
                    loop {
                        match open(&world, &resident, &machine).await {
                            Ok((composing, name, composer)) => {
                                if pieces
                                    .send(Piece::Snapshot { watch: name.clone(), composer: Box::new(composer) })
                                    .is_err()
                                {
                                    return;
                                }
                                break (composing, name);
                            }
                            Err(error) => {
                                if pieces.send(interrupted(again.broke(error.why))).is_err() || error.deleted {
                                    return;
                                }
                                again.pause(&world, machine.as_ref()).await;
                            }
                        }
                    }
                };
                stream(world, resident, machine, composing, name, pieces).await;
            }
        },
    )?;
    Ok(Watching { id, watch, composer })
}

/// Why the shared input could not be opened: `deleted` when its
/// conversation was deleted, and it is opened no more.
struct NotOpened {
    why: String,
    deleted: bool,
}

impl From<String> for NotOpened {
    fn from(why: String) -> Self {
        Self { why, deleted: false }
    }
}

async fn open(
    world: &World,
    resident: &ResidentId,
    machine: &Option<Written>,
) -> Result<(Composing, String, Composer), NotOpened> {
    // A device waits for the link of the hub the resident's machine is on.
    let mut changes = world
        .device()
        .map(|device| device.machine_changes(machine.as_ref()).map(|(_, changes)| changes))
        .transpose()
        .map_err(|error| error.to_string())?;
    loop {
        if let Some(connection) = changes.as_mut() {
            loop {
                let status = connection.borrow_and_update().clone();
                match status {
                    Status::Online { .. } => break,
                    Status::Stopped { reason } => return Err(reason.into()),
                    Status::Connecting { .. } => connection.changed().await.map_err(|error| error.to_string())?,
                }
            }
        }
        #[cfg(target_os = "android")]
        crate::android::readiness::wait(world, machine.as_ref()).await?;
        let name = fastctx_client::composer::watch_id().map_err(|error| error.to_string())?;
        let read = tokio::select! {
            changed = async { match changes.as_mut() { Some(changes) => changes.changed().await.map_err(|error| error.to_string()), None => std::future::pending().await } } => {
                changed?;
                continue;
            },
            read = world.watch_composer(resident.clone(), machine.clone(), name.clone()) => read,
        };
        let (composing, composer) =
            read.map_err(|error| NotOpened { deleted: error.is_deleted(), why: error.to_string() })?;
        return Ok((composing, name, composer));
    }
}

/// Follows the shared input until the page lets go. A watch that broke is
/// not the end (`world-network.md`, 断开不是结束): the page is told it is
/// interrupted and why, and it is opened again as `Again` paces it, under a
/// fresh watch name the next snapshot carries. A hidden Android page lets
/// its watch go until it is shown again.
async fn stream(
    world: World,
    resident: ResidentId,
    machine: Option<Written>,
    initial: Composing,
    name: String,
    pieces: crate::watch_delivery::Sender<Piece>,
) {
    #[cfg(target_os = "android")]
    let mut visible = crate::android::visibility();
    let mut again = fastctx_client::Again::default();
    again.came();
    let mut composing = Some(initial);
    let mut name = name;
    // Whether the last try broke, so the next waits; a page shown again
    // opens at once.
    let mut broken = false;
    loop {
        #[cfg(target_os = "android")]
        if !*visible.borrow_and_update() {
            composing = None;
        }
        let next = tokio::select! {
            _ = shown(
                #[cfg(target_os = "android")]
                &mut visible,
            ) => None,
            next = async { match composing.as_mut() { Some(composing) => composing.next().await, None => std::future::pending().await } } => Some(next),
        };
        match next {
            Some(Ok(Some(composer))) => {
                if pieces.send(Piece::Snapshot { watch: name.clone(), composer: Box::new(composer) }).is_err() {
                    return;
                }
                continue;
            }
            Some(ended) => {
                drop(composing.take());
                broken = true;
                let (why, deleted) = match ended {
                    Ok(_) => ("the shared input's watch ended".to_string(), false),
                    Err(error) => (error.to_string(), error.is_deleted()),
                };
                // Its conversation deleted, there is nothing to open again.
                if pieces.send(interrupted(again.broke(why))).is_err() || deleted {
                    return;
                }
            }
            None => drop(composing.take()),
        }
        loop {
            #[cfg(target_os = "android")]
            if !*visible.borrow_and_update() {
                break;
            }
            if broken {
                tokio::select! {
                    () = again.pause(&world, machine.as_ref()) => {}
                    _ = shown(
                        #[cfg(target_os = "android")]
                        &mut visible,
                    ) => {}
                }
            }
            match open(&world, &resident, &machine).await {
                Ok((fresh, watch, composer)) => {
                    again.came();
                    broken = false;
                    name = watch;
                    if pieces.send(Piece::Snapshot { watch: name.clone(), composer: Box::new(composer) }).is_err() {
                        return;
                    }
                    composing = Some(fresh);
                    break;
                }
                Err(not) => {
                    broken = true;
                    if pieces.send(interrupted(again.broke(not.why))).is_err() || not.deleted {
                        return;
                    }
                }
            }
        }
    }
}

fn interrupted(interruption: fastctx_client::Interruption) -> Piece {
    Piece::Interrupted { error: Some(interruption.why), since: crate::epoch_ms(interruption.since) }
}

/// A change of whether an Android page is shown; never elsewhere.
async fn shown(#[cfg(target_os = "android")] visible: &mut tokio::sync::watch::Receiver<bool>) {
    #[cfg(target_os = "android")]
    {
        if visible.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
    #[cfg(not(target_os = "android"))]
    std::future::pending::<()>().await
}

/// A completed upload receipt is returned even when the draft changed during transfer.
#[derive(Serialize)]
pub struct AttachmentAdded {
    attachment: Attachment,
    updated: Result<Updated, String>,
}

#[fastctx_gui_command::command]
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
    crate::android::readiness::wait(&world, machine.as_ref()).await?;
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

#[fastctx_gui_command::command]
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
    crate::android::readiness::wait(&world, machine.as_ref()).await?;
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
