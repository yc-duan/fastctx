//! Native host lifetimes survive node restart; an absent connection is not an exit.
use crate::records;
use fastctx_types::{
    ProcessIdentity, ResidentId,
    layout::{ConversationDir, WorldDir},
};
use std::io;

fn identity(process: &ProcessIdentity) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(serde_json::to_vec(process).expect("process identity serializes")))
}
pub(crate) fn prepare(world: &WorldDir, id: &ResidentId) -> io::Result<ConversationDir> {
    let directory = world.conversation(id);
    let path = directory.path().join("native-hosts.json");
    records::ensure_file(&directory, "native-processes", path.clone(), |path, push| {
        records::legacy::raw_array::<ProcessIdentity>(path, Some("processes"), &mut |process, body| {
            push(records::Source::new(identity(&process), path.to_path_buf(), body)?)
        })
    })?;
    records::ensure_file(&directory, "native-lifetime", path, |path, push| {
        if let Some(unknown) = records::legacy::field::<bool>(path, "unknown")? {
            push(records::Source::new("unknown".into(), path.to_path_buf(), serde_json::to_vec(&unknown)?)?)?;
        }
        Ok(())
    })?;
    Ok(directory)
}
pub(crate) fn record(world: &WorldDir, id: &ResidentId, process: Option<ProcessIdentity>) -> io::Result<()> {
    let directory = prepare(world, id)?;
    let prior = records::read(&directory, |rows| rows.all::<ProcessIdentity>("native-processes"))?;
    let ended: Vec<_> = prior
        .into_iter()
        .filter(|process| matches!(fastctx_platform::has_ended(process), Ok(true)))
        .map(|process| identity(&process))
        .collect();
    for group in ended.chunks(16) {
        let group = group.to_vec();
        records::write_at(&directory, fastctx_storage::Priority::Normal, 1024, move |rows| {
            for key in group {
                rows.delete("native-processes", &key)?;
            }
            Ok(())
        })?;
    }
    records::write_at(&directory, fastctx_storage::Priority::Interactive, 512, move |rows| {
        rows.put("native-lifetime", "unknown", &process.is_none())?;
        if let Some(process) = process {
            rows.put("native-processes", &identity(&process), &process)?;
        }
        Ok(())
    })
}
/// Missing or unreadable lifetime evidence is an explicit unavailable result.
pub(crate) fn processes(world: &WorldDir, id: &ResidentId) -> Result<Vec<ProcessIdentity>, String> {
    let directory = prepare(world, id).map_err(|error| error.to_string())?;
    let (unknown, processes) = records::read(&directory, |rows| {
        Ok((
            rows.get::<bool>("native-lifetime", "unknown")?.unwrap_or(true),
            rows.all::<ProcessIdentity>("native-processes")?,
        ))
    })
    .map_err(|error| error.to_string())?;
    if unknown || processes.is_empty() {
        return Err(
            "the native host lifetime is unknown; reconnect with updated World tools before background resumption"
                .into(),
        );
    }
    Ok(processes)
}
pub(crate) fn closed(world: &WorldDir, id: &ResidentId, controlled: Option<&ProcessIdentity>) -> Result<bool, String> {
    for process in processes(world, id)? {
        if controlled == Some(&process) {
            continue;
        }
        if !fastctx_platform::has_ended(&process)
            .map_err(|error| format!("checking the native host lifetime: {error}"))?
        {
            return Ok(false);
        }
    }
    Ok(true)
}
