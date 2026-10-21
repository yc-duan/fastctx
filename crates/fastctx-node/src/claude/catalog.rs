//! A session's capabilities outlive its mod process.

use crate::Node;
use fastctx_hosts::claude::control as files;
use fastctx_types::ResidentId;
use fastctx_types::ipc::Failure;
use fastctx_types::session::control::{ControlKind, Offers};
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
struct Catalog {
    instance: Option<String>,
    offers: Offers,
}

pub(super) fn keep(node: &Node, resident: &ResidentId, offers: &Offers) -> Result<(), Failure> {
    let session = node.residents.about(resident).and_then(|(about, _)| about.session);
    let host = session
        .as_deref()
        .map(|session| files::host(&files::directory(node.world.path(), session)))
        .transpose()
        .map_err(failure)?
        .flatten();
    let catalog = Catalog { instance: host.as_ref().map(|host| host.instance.clone()), offers: offers.clone() };
    files::write(&node.world.conversation(resident).path().join("claude-catalog.json"), &catalog).map_err(failure)?;
    if let (Some(session), Some(host)) = (session, host) {
        collect(&files::directory(node.world.path(), &session), &host.instance).map_err(failure)?;
    }
    Ok(())
}

pub(super) fn get(node: &Node, resident: &ResidentId, session: Option<&str>) -> Result<Offers, Failure> {
    let saved: Option<Catalog> =
        files::read(&node.world.conversation(resident).path().join("claude-catalog.json")).map_err(failure)?;
    if let Some(session) = session {
        let directory = files::directory(node.world.path(), session);
        if let Some(host) = files::host(&directory).map_err(failure)? {
            if saved.as_ref().is_some_and(|saved| saved.instance.as_ref() == Some(&host.instance)) {
                return Ok(saved.expect("checked catalog").offers);
            }
            let path = directory.join(format!("catalog-{}.json", host.instance));
            if path.with_extension("done").try_exists().map_err(failure)? {
                let value = files::read_message(&path).map_err(failure)?;
                let offers = serde_json::from_value(value).map_err(|error| Failure::new(error.to_string()))?;
                keep(node, resident, &offers)?;
                return Ok(offers);
            }
        }
    }
    if let Some(saved) = saved {
        return Ok(saved.offers);
    }
    Ok(Offers {
        controls: vec![
            ControlKind::Command,
            ControlKind::Compact,
            ControlKind::Model,
            ControlKind::Rename,
            ControlKind::Rewind,
            ControlKind::Agents,
            ControlKind::AgentMessages,
        ],
        commands: fastctx_hosts::claude::catalog::commands(),
        ..Default::default()
    })
}

/// The newest activation's catalog stays available without retaining every past activation.
pub(super) fn collect(directory: &std::path::Path, instance: &str) -> std::io::Result<()> {
    for path in crate::retention::entries(directory)? {
        let Some(stem) = path.file_stem().and_then(|name| name.to_str()) else { continue };
        if let Some(saved) = stem.strip_prefix("catalog-")
            && saved != instance
            && saved.bytes().all(|byte| byte.is_ascii_digit())
        {
            fastctx_platform::owned_directory::delete(&path)?;
        }
    }
    Ok(())
}

fn failure(error: std::io::Error) -> Failure {
    Failure::new(format!("reading Claude's command catalog: {error}"))
}
