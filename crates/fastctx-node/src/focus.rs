//! The machines and folders the user points a conversation to (`world-gui.md`,
//! 新对话 and 对话): kept in its record and told to the model apart from the
//! user's words, each folder with what is in it as the machine holding it
//! reads it then. A Codex thread is told in its instructions and a Claude
//! session in a block of context its plugin adds to the first message; both
//! hosts put those back after compacting the conversation, so neither is
//! told again then (`world-colleagues.md`, 这段对话涉及的机器与文件夹). A
//! change is told as the World's words. A Codex thread keeps the
//! instructions it was started with, so once a change outdates them it is
//! told the current ones again after each compaction; a Claude session's
//! block is read anew at each compaction and is current then.
//!
//! Nothing here stops a conversation from starting or a change from being
//! made: a machine or folder that cannot be read is told as such.

use fastctx_types::access::Written;
use std::sync::Arc;
use std::time::Duration;

use fastctx_types::access::{Acting, Purpose};
use fastctx_types::focus::{Focus, FocusedMachine, Folders, Outlined};
use fastctx_types::ipc::Failure;
use fastctx_types::layout::ConversationDir;
use fastctx_types::remote::{RemoteCall, RemoteReply};
use fastctx_types::residents::{About, Delivery, Notice, Origin, WorldFact};
use fastctx_types::{MachineName, ResidentId};
use jiff::Timestamp;
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
    tokio::task::spawn_blocking(move || fastctx_hands::folders::list(machine, path.as_deref()))
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
            tokio::task::spawn_blocking(move || fastctx_hands::outline::outline(&path, deadline))
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

/// What the model is told of `focus`, read now; none when it points to
/// nothing. The session's shell is on this machine, whoever it works for.
pub(crate) async fn note(node: &Arc<Node>, acting: &Acting, focus: &Focus) -> Option<String> {
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
    for (machine, outlines) in focus.machines.iter().zip(read) {
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
    Some(text)
}

fn indent(outline: &str) -> String {
    outline.lines().map(|line| format!("  {line}")).collect::<Vec<_>>().join("\n")
}

/// What the folders of one machine hold, read there.
async fn read(node: &Arc<Node>, acting: &Acting, machine: &FocusedMachine) -> Result<Vec<Outlined>, String> {
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
    fastctx_platform::ensure_private_dir(directory.path())
        .map_err(|error| Failure::new(format!("making the conversation's directory: {error}")))?;
    let note = note(node, acting, focus).await;
    keep(directory, note.as_deref())?;
    let opening = Opening { focus: focus.clone(), note: note.unwrap_or_default() };
    let bytes = serde_json::to_vec_pretty(&opening).map_err(|error| Failure::new(error.to_string()))?;
    write(&directory.focus_opening(), &bytes)
}

/// What a Codex thread's instructions say of its machines and folders,
/// after the rest.
pub(crate) fn opening(directory: &ConversationDir) -> Result<Option<Opening>, Failure> {
    match std::fs::read(directory.focus_opening()) {
        Ok(bytes) => serde_json::from_slice(&bytes).map(Some).map_err(|error| Failure::new(error.to_string())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(Failure::new(format!("reading what the instructions say of its folders: {error}"))),
    }
}

/// Keeps what the model is told of the machines and folders now, where a
/// Claude session's plugin reads it.
fn keep(directory: &ConversationDir, note: Option<&str>) -> Result<(), Failure> {
    match note {
        Some(note) => write(&directory.focus(), note.as_bytes()),
        None => match std::fs::remove_file(directory.focus()) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                Err(Failure::new(format!("forgetting its folders: {error}")))
            }
            _ => Ok(()),
        },
    }
}

fn write(path: &std::path::Path, bytes: &[u8]) -> Result<(), Failure> {
    let parent = path.parent().expect("a conversation's books");
    fastctx_platform::ensure_private_dir(parent)
        .and_then(|()| fastctx_platform::publish(path, bytes, fastctx_platform::Permissions::Private))
        .map_err(|error| Failure::new(format!("keeping its folders: {error}")))
}

/// Puts what the model is told of the machines and folders now beside a
/// Claude session's own files, where its plugin reads it into the first
/// message's context: when the session is reserved, and when they change.
pub(crate) fn hand_to_session(directory: &ConversationDir, session: &std::path::Path) -> std::io::Result<()> {
    let target = session.join("focus.md");
    match std::fs::read(directory.focus()) {
        Ok(bytes) => fastctx_platform::publish(&target, &bytes, fastctx_platform::Permissions::Private),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => match std::fs::remove_file(&target) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        },
        Err(error) => Err(error),
    }
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
pub(crate) async fn changed(
    node: &Arc<Node>,
    id: &ResidentId,
    about: &About,
    focus: &Focus,
) -> Result<Delivery, Failure> {
    let directory = node.world.conversation(id);
    let note = note(node, &about.acting, focus).await;
    keep(&directory, note.as_deref())?;
    if about.host.agent.as_str() == "claude"
        && let Some(session) = &about.session
    {
        let exchange = fastctx_hosts::claude::exchange::sessions(node.world.path()).join(session);
        if exchange.is_dir()
            && let Err(error) = hand_to_session(&directory, &exchange)
        {
            node.log.line(format_args!("handing {id}'s folders to its session: {error}"));
        }
    }
    let text = match note {
        Some(note) => format!(
            "The user changed the machines and folders of this conversation; this replaces what you were told of them before.\n\n{note}"
        ),
        None => "The user cleared the machines and folders of this conversation; what you were told of them before no longer holds.".to_string(),
    };
    Ok(Delivery {
        id: crate::speech::new_id("focus"),
        at: Timestamp::now(),
        notice: Notice::World { fact: WorldFact::Focus { focus: focus.clone() }, text },
    })
}

/// A fork keeps context already injected into its source's history. Its own
/// instructions apply on the next full context rebuild, so its current
/// selection is also delivered once with the next turn, without waking it.
pub(crate) async fn forked(node: &Arc<Node>, id: &ResidentId) -> Result<(), Failure> {
    let opening = opening(&node.world.conversation(id))?
        .ok_or_else(|| Failure::new("the fork has no saved machine and folder context"))?;
    let text = if opening.note.is_empty() {
        "No machines or folders are selected for this fork; this replaces any earlier selection in its inherited history and does not limit which machines it may use.".to_string()
    } else {
        format!(
            "The machines and folders selected for this fork; this replaces any earlier selection in its inherited history.\n\n{}",
            opening.note
        )
    };
    let delivery = Delivery {
        id: crate::speech::new_id("fork-focus"),
        at: Timestamp::now(),
        notice: Notice::World { fact: WorldFact::Focus { focus: opening.focus }, text },
    };
    crate::speech::tell_here(node, id, delivery, crate::speech::Owed::Nobody).await?;
    Ok(())
}

/// A Codex thread compacted its conversation, `item` naming the compaction:
/// it is told the machines and folders again where its instructions are
/// out of date.
pub(crate) async fn compacted(node: &Arc<Node>, id: &ResidentId, item: &str) {
    let Some((about, _)) = node.residents.about(id) else { return };
    let directory = node.world.conversation(id);
    let opened = match opening(&directory) {
        Ok(opened) => opened.map(|opened| opened.focus).unwrap_or_default(),
        Err(failure) => {
            node.log.line(format_args!("after {id} compacted: {}", failure.message));
            return;
        }
    };
    if opened == about.focus {
        return;
    }
    let note = note(node, &about.acting, &about.focus).await;
    if let Err(failure) = keep(&directory, note.as_deref()) {
        node.log.line(format_args!("after {id} compacted: {}", failure.message));
    }
    let text = match note {
        Some(note) => format!(
            "The machines and folders of this conversation, as the user last set them; this replaces what your instructions say of them.\n\n{note}"
        ),
        None => "The user cleared the machines and folders of this conversation; what your instructions say of them no longer holds.".to_string(),
    };
    let delivery = Delivery {
        // The same compaction told twice is told once.
        id: crate::speech::fixed_id("focus-compacted", &[item]),
        at: Timestamp::now(),
        notice: Notice::World { fact: WorldFact::Focus { focus: about.focus.clone() }, text },
    };
    if let Err(failure) = crate::speech::tell_here(node, id, delivery, crate::speech::Owed::Nobody).await {
        node.log.line(format_args!("telling {id} its folders after it compacted: {}", failure.message));
    }
}
