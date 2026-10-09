//! Whether a resident may ask for colleagues, and where they live
//! (`world-colleagues.md`, 同事从哪里来、能去哪里): off until the user turns
//! it on for that conversation. Kept by resident id, so it follows the
//! resident from town to town.

use std::io;
use std::path::PathBuf;

use fastctx_platform::{Permissions, ensure_private_dir, publish};
use fastctx_types::ResidentId;
use fastctx_types::layout::WorldDir;
use fastctx_types::access::Written;
use fastctx_types::ipc::Failure;
use fastctx_types::residents::Origin;
use serde::{Deserialize, Serialize};

use crate::Node;

#[derive(Serialize, Deserialize)]
struct Setting {
    lives_on: Written,
    /// The Codex home there they use, when not the default one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    home: Option<PathBuf>,
}

fn path(world: &WorldDir, id: &ResidentId) -> PathBuf {
    world.path().join("colleagues").join(format!("{id}.json"))
}

/// Where `id`'s colleagues live, when it may ask for them.
pub fn read(node: &Node, id: &ResidentId) -> io::Result<Option<Written>> {
    read_in(&node.world, id)
}

/// The same, read from the World's directory alone.
pub(crate) fn read_in(world: &WorldDir, id: &ResidentId) -> io::Result<Option<Written>> {
    Ok(setting(world, id)?.map(|setting| setting.lives_on))
}

/// The Codex home `id`'s colleagues use where they live, when it may ask
/// for them and it is not the default one there.
pub fn home(node: &Node, id: &ResidentId) -> io::Result<Option<PathBuf>> {
    home_in(&node.world, id)
}

/// The same, read from the World's directory alone.
pub(crate) fn home_in(world: &WorldDir, id: &ResidentId) -> io::Result<Option<PathBuf>> {
    Ok(setting(world, id)?.and_then(|setting| setting.home))
}

fn setting(world: &WorldDir, id: &ResidentId) -> io::Result<Option<Setting>> {
    match std::fs::read(path(world, id)) {
        Ok(bytes) => serde_json::from_slice::<Setting>(&bytes)
            .map(Some)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// Turns asking for colleagues on for `id`, a resident of this machine,
/// with where they live and in which Codex home there, or off. A colleague
/// never asks for colleagues.
pub fn set(node: &Node, id: &ResidentId, lives_on: Option<Written>, home: Option<PathBuf>) -> Result<(), Failure> {
    let (about, _) = node.residents.about(id).ok_or_else(|| Failure::not_run(format!("no resident {id} lives here")))?;
    if let Origin::Colleague { asked_by, .. } = &about.origin {
        return Err(Failure::not_run(format!(
            "{id} is a colleague {asked_by} asked for; a colleague does not ask for colleagues"
        )));
    }
    match &lives_on {
        Some(lives_on) => keep(node, id, lives_on, home)?,
        None => {
            let path = path(&node.world, id);
            match std::fs::remove_file(&path) {
                Err(error) if error.kind() != io::ErrorKind::NotFound => {
                    return Err(Failure::new(format!("removing the colleague setting {}: {error}", path.display())));
                }
                _ => {}
            }
        }
    }
    node.tools_changed(Some(id));
    Ok(())
}

/// A fork of `parent` asks for colleagues as `parent` does.
pub fn inherit(node: &Node, parent: &ResidentId, id: &ResidentId) -> Result<(), Failure> {
    match setting(&node.world, parent) {
        Ok(Some(setting)) => keep(node, id, &setting.lives_on, setting.home),
        Ok(None) => Ok(()),
        Err(error) => Err(Failure::new(format!("reading {parent}'s colleague setting: {error}"))),
    }
}

/// Keeps where `id`'s colleagues live, for a resident being born.
pub fn keep(node: &Node, id: &ResidentId, lives_on: &Written, home: Option<PathBuf>) -> Result<(), Failure> {
    let path = path(&node.world, id);
    let setting = Setting { lives_on: lives_on.clone(), home };
    let mut bytes = serde_json::to_vec_pretty(&setting).expect("a setting serializes");
    bytes.push(b'\n');
    ensure_private_dir(path.parent().expect("the colleagues directory"))
        .and_then(|()| publish(&path, &bytes, Permissions::Private))
        .map_err(|error| Failure::new(format!("keeping the colleague setting in {}: {error}", path.display())))
}
