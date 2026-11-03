//! The user's machine and folder selection, with outlines read by each
//! machine holding those folders. Named context owns delivery and compaction
//! recovery; these functions only maintain the current authorized selection.
//! Unreadable machines or folders are described as such in that context.

pub(crate) mod storage;
use fastctx_platform::task;
use fastctx_types::access::Written;
use std::sync::Arc;
use std::time::Duration;

use fastctx_types::access::{Acting, Purpose};
use fastctx_types::focus::{Focus, FocusedMachine, Folders, Outlined};
use fastctx_types::ipc::Failure;
use fastctx_types::layout::ConversationDir;
use fastctx_types::remote::{RemoteCall, RemoteReply};
use fastctx_types::residents::{About, Origin};
use fastctx_types::{MachineName, ResidentId};
use serde::{Deserialize, Serialize};

use crate::Node;

/// How long reading the folders may take, as the user set it: what was
/// read by then is told, so a slow disk or machine never holds up a
/// conversation's start, a change or its compaction.
const READING: Duration = Duration::from_secs(5);

/// The part of it another machine reading its own folders has, the rest
/// being the way there and back.
const READING_THERE: Duration = Duration::from_secs(4);

/// What a Codex thread's instructions say of the machines and folders: the
/// ones they were written for, and the words.
#[derive(Serialize, Deserialize)]
pub(crate) struct Opening {
    pub(crate) focus: Focus,
    pub(crate) note: String,
}

/// The folders in a folder of `machine`, or of this one, for the user
/// choosing one.
pub(crate) async fn folders(
    node: &Arc<Node>,
    acting: &Acting,
    machine: Option<Written>,
    path: Option<String>,
) -> Result<Folders, Failure> {
    let here = node.machine();
    let Some(target) = machine.filter(|machine| !node.is_here(machine)) else {
        node.authorize_here(acting)?;
        return folders_here(here, path).await;
    };
    match node
        .call(acting, &target, Purpose::Hands, RemoteCall::Folders { path })
        .await
        .map_err(|failure| Failure::new(failure.to_string()))??
    {
        RemoteReply::Folders(folders) => Ok(folders),
        _ => Err(Failure::new(format!("{target} answered something other than its folders"))),
    }
}

pub(crate) async fn folders_here(machine: MachineName, path: Option<String>) -> Result<Folders, Failure> {
    task::spawn_blocking(move || fastctx_hands::folders::list(machine, path.as_deref()))
        .await
        .map_err(|error| Failure::new(error.to_string()))?
}

/// What each folder of this machine holds, read within `within`: each
/// folder on its own, so one stuck on a slow disk keeps the rest from
/// nothing; what its reading has not given by then is said so.
pub(crate) async fn outline_here(folders: Vec<String>, within: Duration) -> Vec<Outlined> {
    let started = tokio::time::Instant::now();
    let deadline = std::time::Instant::now() + within;
    let reading: Vec<_> = folders
        .iter()
        .map(|path| {
            let path = path.clone();
            task::spawn_blocking(move || fastctx_hands::outline::outline(&path, deadline))
        })
        .collect();
    let mut outlined = Vec::with_capacity(folders.len());
    for (path, reading) in folders.into_iter().zip(reading) {
        let outline = match tokio::time::timeout_at(started + within, reading).await {
            Ok(Ok(outline)) => outline,
            Ok(Err(error)) => Err(format!("reading it stopped: {error}")),
            Err(_) => Err(format!("it was not read within {} seconds", within.as_secs())),
        };
        outlined.push(Outlined { path, outline });
    }
    outlined
}

/// The part of a selection the subject may use, before names or contents are disclosed.
pub(crate) fn allowed(node: &Node, acting: &Acting, focus: &Focus) -> Focus {
    Focus {
        machines: focus
            .machines
            .iter()
            .filter(|machine| node.authorize_machine(acting, &machine.machine).is_ok())
            .cloned()
            .collect(),
    }
}

/// What the model is told of the authorized selection, read now.
pub(crate) async fn note(node: &Arc<Node>, acting: &Acting, focus: &Focus) -> Option<String> {
    let focus = allowed(node, acting, focus);
    if focus.is_empty() {
        return None;
    }
    // Every machine is read at once: one slow to answer delays only itself.
    let mut reading = tokio::task::JoinSet::new();
    for (index, machine) in focus.machines.iter().enumerate() {
        let (node, acting, machine) = (node.clone(), acting.clone(), machine.clone());
        reading.spawn(async move { (index, read(&node, &acting, &machine).await) });
    }
    let mut read = vec![None; focus.machines.len()];
    while let Some(done) = reading.join_next().await {
        if let Ok((index, outlines)) = done {
            read[index] = Some(outlines);
        }
    }
    let mut text = String::from(
        "The user pointed this conversation at these machines and folders: they are where its work is. They do not limit which machines you may use. Each folder is shown one level deep, as it was when this was written: folders with how many entries they hold, files with their lines, or their size when not text.",
    );
    let mut shown = false;
    for (machine, outlines) in focus.machines.iter().zip(read) {
        if node.authorize_machine(acting, &machine.machine).is_err() {
            continue;
        }
        shown = true;
        text.push_str("\n\n");
        text.push_str(&machine.machine.to_string());
        if machine.folders.is_empty() {
            text.push_str(": the machine as a whole");
            continue;
        }
        match outlines {
            Some(Ok(outlines)) => {
                for outlined in outlines {
                    text.push('\n');
                    match outlined.outline {
                        Ok(outline) => text.push_str(&indent(&outline)),
                        Err(why) => text.push_str(&format!("  {} (could not be read: {why})", outlined.path)),
                    }
                }
            }
            Some(Err(why)) => {
                text.push_str(&format!(": its folders could not be read: {why}"));
                for folder in &machine.folders {
                    text.push_str(&format!("\n  {folder}"));
                }
            }
            None => {
                text.push_str(": reading its folders stopped");
                for folder in &machine.folders {
                    text.push_str(&format!("\n  {folder}"));
                }
            }
        }
    }
    shown.then_some(text)
}

fn indent(outline: &str) -> String {
    outline.lines().map(|line| format!("  {line}")).collect::<Vec<_>>().join("\n")
}

/// What the folders of one machine hold, read there.
async fn read(node: &Arc<Node>, acting: &Acting, machine: &FocusedMachine) -> Result<Vec<Outlined>, String> {
    node.settled(&machine.machine).await;
    node.authorize_machine(acting, &machine.machine).map_err(|failure| failure.message)?;
    if machine.folders.is_empty() {
        return Ok(Vec::new());
    }
    if node.is_here(&machine.machine) {
        return Ok(outline_here(machine.folders.clone(), READING).await);
    }
    let call = RemoteCall::Outline { folders: machine.folders.clone(), within_ms: READING_THERE.as_millis() as u64 };
    match tokio::time::timeout(READING, node.call(acting, &machine.machine, Purpose::Hands, call)).await {
        Ok(Ok(Ok(RemoteReply::Outlined { outlines }))) => Ok(outlines),
        Ok(Ok(Ok(_))) => Err("it answered something else".to_string()),
        Ok(Ok(Err(failure))) => Err(failure.message),
        Ok(Err(failure)) => Err(failure.to_string()),
        Err(_) => Err(format!("it did not answer within {} seconds", READING.as_secs())),
    }
}

/// What a conversation opened with `focus` is told of it from its first
/// turn on, kept with it: for Codex in the instructions its thread starts
/// with, for Claude in its plugin's block of context.
pub(crate) async fn open(
    node: &Arc<Node>,
    directory: &ConversationDir,
    acting: &Acting,
    focus: &Focus,
) -> Result<(), Failure> {
    // A refresh begun meanwhile (`refresh`) works the same focus out from
    // the conversation's own and keeps it first; what is opened with is
    // then worked out again, so the conversation never starts without it.
    for _ in 0..3 {
        let room = directory.clone();
        let selection = allowed(node, acting, focus);
        let generation =
            task::spawn_blocking(move || storage::begin(&room, &selection)).await.map_err(failed)?.map_err(failed)?;
        let note = note(node, acting, focus).await;
        let visible = allowed(node, acting, focus);
        let opening = Opening { focus: visible.clone(), note: note.clone().unwrap_or_default() };
        let room = directory.clone();
        let kept =
            task::spawn_blocking(move || storage::keep(&room, generation, note.as_deref(), &visible, Some(&opening)))
                .await
                .map_err(failed)?
                .map_err(failed)?;
        if kept {
            return Ok(());
        }
    }
    Err(Failure::new("what the conversation is pointed at kept changing while it was opened"))
}

fn failed(error: impl std::fmt::Display) -> Failure {
    Failure::new(error.to_string())
}

/// The opening selection is read on a worker, including before resident creation.
pub(crate) async fn opening(directory: &ConversationDir) -> Result<Option<Opening>, Failure> {
    let directory = directory.clone();
    task::spawn_blocking(move || storage::opening(&directory)).await.map_err(failed)?.map_err(failed)
}

pub(crate) fn text(directory: &ConversationDir) -> std::io::Result<String> {
    storage::text(directory)
}

/// Replaces cached context when authority changes, without rewriting the user's selection.
pub(crate) async fn refresh(node: &Arc<Node>, id: &ResidentId) -> Result<(), Failure> {
    refresh_selection(node, id, false).await
}

async fn refresh_selection(node: &Arc<Node>, id: &ResidentId, force: bool) -> Result<(), Failure> {
    let (working, resident) = (node.clone(), id.clone());
    let pending = task::spawn_blocking(move || -> std::io::Result<_> {
        let Some((about, _)) = working.residents.about(&resident) else { return Ok(None) };
        let directory = working.world.conversation(&resident);
        let visible = allowed(&working, &about.acting, &about.focus);
        if !force && storage::visible(&directory)?.as_ref() == Some(&visible) {
            return Ok(None);
        }
        let generation = storage::begin(&directory, &visible)?;
        Ok(Some((about, visible, generation)))
    })
    .await
    .map_err(failed)?
    .map_err(failed)?;
    let Some((about, visible, generation)) = pending else { return Ok(()) };
    let note = note(node, &about.acting, &visible).await;
    let (working, resident) = (node.clone(), id.clone());
    task::spawn_blocking(move || -> std::io::Result<()> {
        let Some((current, _)) = working.residents.about(&resident) else { return Ok(()) };
        if allowed(&working, &current.acting, &current.focus) != visible {
            return Ok(());
        }
        if storage::keep(&working.world.conversation(&resident), generation, note.as_deref(), &visible, None)? {
            crate::context::sources::refresh_part_now(&working, &resident, crate::context::parts::Part::Focus)?;
        }
        Ok(())
    })
    .await
    .map_err(failed)?
    .map_err(failed)
}

/// Whether the user may point this conversation at machines and folders:
/// one they opened from fastctx.
pub(crate) fn pointable(about: &About) -> Result<(), Failure> {
    if !matches!(about.origin, Origin::User) || !about.world_started {
        return Err(Failure::not_run(
            "only a conversation the user opened from fastctx is pointed at machines and folders",
        ));
    }
    Ok(())
}

/// The user pointed the conversation at `focus`, which is now in its
/// record: what it is told of them is kept, and the change told to it.
pub(crate) async fn changed(node: &Arc<Node>, id: &ResidentId) -> Result<(), Failure> {
    refresh_selection(node, id, true).await
}

/// A fork's inherited history may contain a different selection. Queue its
/// current context for the next tool response without starting a turn.
pub(crate) async fn forked(node: &Arc<Node>, id: &ResidentId) -> Result<(), Failure> {
    let (node, id) = (node.clone(), id.clone());
    task::spawn_blocking(move || crate::context::forked(&node, &id)).await.map_err(failed)?.map_err(failed)
}
