//! The files a conversation changed, as the machine it lives on keeps them
//! (`conversation.files`), read for the conversations this window watches.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use fastctx_client::World;
use fastctx_types::ResidentId;
use fastctx_types::access::Written;
use fastctx_types::timeline_pages::{FILES_ITEMS, FileChangeTold, TimelineAsk, TimelineTold};

/// How long one reading may take before the page is told to ask again.
const WITHIN: Duration = Duration::from_secs(30);

/// Whom a watch is of: the World it is read in, the conversation, and the
/// machine it was asked of.
type Whom = (World, ResidentId, Option<Written>);

/// Whom each watch is of, by the watch's id.
#[derive(Default)]
pub(crate) struct Watched(Mutex<HashMap<u64, Whom>>);

impl Watched {
    pub(crate) fn open(&self, id: u64, world: World, resident: ResidentId, machine: Option<Written>) {
        self.0.lock().expect("watched conversations").insert(id, (world, resident, machine));
    }

    pub(crate) fn close(&self, id: u64) {
        self.0.lock().expect("watched conversations").remove(&id);
    }

    pub(crate) fn clear(&self) {
        self.0.lock().expect("watched conversations").clear();
    }

    fn of(&self, id: u64) -> Option<Whom> {
        self.0.lock().expect("watched conversations").get(&id).cloned()
    }
}

/// Those written after `after`, the place an earlier reading gave ("" for
/// all), told as text, for it takes all of 64 bits. Not known from a machine
/// that keeps none, as one of an earlier release does: the page reads them
/// from its calls then.
#[derive(serde::Serialize)]
pub(crate) struct FileChanges {
    known: bool,
    /// The place given means nothing where they were read: read them all
    /// again.
    reread: bool,
    changes: Vec<FileChangeTold>,
    through: String,
    more: bool,
}

#[fastctx_gui_command::command]
pub(crate) async fn conversation_files(
    shell: tauri::State<'_, crate::Shell>,
    id: u64,
    after: String,
) -> Result<FileChanges, String> {
    let unknown = FileChanges { known: false, reread: false, changes: Vec::new(), through: String::new(), more: false };
    let (world, resident, machine) = shell.watched.of(id).ok_or("the conversation is no longer watched")?;
    let after = if after.is_empty() { 0 } else { after.parse::<u64>().map_err(|error| error.to_string())? };
    let reading = world.timeline(machine, TimelineAsk::Files { resident, after, max_items: FILES_ITEMS });
    match tokio::time::timeout(WITHIN, reading).await {
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
