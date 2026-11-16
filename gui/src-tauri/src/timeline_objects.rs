//! Native command for explicitly expanding one oversized timeline event.

#[derive(serde::Serialize)]
pub(crate) struct ExpandedEvent {
    object: String,
    event: Box<serde_json::value::RawValue>,
}

#[fastctx_gui_command::command]
pub(crate) async fn conversation_object(
    shell: tauri::State<'_, crate::Shell>,
    id: u64,
    generation: u64,
    object: String,
) -> Result<ExpandedEvent, String> {
    let reader = shell.timeline_readers.get(id).ok_or("the conversation is no longer watched")?;
    let event = crate::timeline_body::read(reader, generation, object.clone()).await?;
    Ok(ExpandedEvent { object, event })
}

/// The files a conversation changed as the machine it lives on keeps them
/// (`conversation.files`): those written after `after`, the place an
/// earlier reading gave ("" for all), told as text, for it takes all of 64
/// bits. Not known from a machine that keeps none, as one of an earlier
/// release does, nor where it cannot be asked now: the page reads them from
/// its calls then.
#[derive(serde::Serialize)]
pub(crate) struct FileChanges {
    known: bool,
    /// The place given means nothing where they were read: read them all
    /// again.
    reread: bool,
    changes: Vec<fastctx_types::timeline_pages::FileChangeTold>,
    through: String,
    more: bool,
}

#[fastctx_gui_command::command]
pub(crate) async fn conversation_files(
    shell: tauri::State<'_, crate::Shell>,
    id: u64,
    after: String,
) -> Result<FileChanges, String> {
    use fastctx_types::timeline_pages::{FILES_ITEMS, TimelineAsk, TimelineTold};
    let reader = shell.timeline_readers.get(id).ok_or("the conversation is no longer watched")?;
    let (world, machine, resident) = {
        let reader = reader.lock().await;
        (reader.world.clone(), reader.machine.clone(), reader.resident.clone())
    };
    let after = if after.is_empty() { 0 } else { after.parse::<u64>().map_err(|error| error.to_string())? };
    let unknown = FileChanges { known: false, reread: false, changes: Vec::new(), through: String::new(), more: false };
    let reading = world.timeline(machine, TimelineAsk::Files { resident, after, max_items: FILES_ITEMS });
    let told = tokio::select! {
        told = tokio::time::timeout(std::time::Duration::from_secs(30), reading) => told,
        () = reader.closed() => return Err("the conversation window closed".into()),
    };
    match told {
        Ok(Ok(TimelineTold::Files { known, changes, through, more })) => {
            Ok(FileChanges { known, reread: false, changes, through: through.to_string(), more })
        }
        Ok(Ok(TimelineTold::Reread)) => Ok(FileChanges { known: true, reread: true, more: true, ..unknown }),
        // A node, machine or hub of an earlier release, never asked.
        Ok(Err(fastctx_client::Error::Call(fastctx_client::CallError::Failed(failure)))) if failure.not_run => Ok(unknown),
        Ok(Ok(_)) => Ok(unknown),
        // Not now: the page asks again a while on.
        Ok(Err(error)) => Err(error.to_string()),
        Err(_) => Err("reading the files the conversation changed took over 30 s".into()),
    }
}
