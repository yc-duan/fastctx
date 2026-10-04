//! Keep the public settings images around CLI mutations as well as around our
//! own file transaction. No private Claude registry or login state is read.

use crate::transaction::Snapshot;
use std::io;
use std::path::{Path, PathBuf};

pub(crate) struct History {
    directory: PathBuf,
    pub before: Snapshot,
}

impl History {
    pub fn begin(root: &Path, settings: &Path, command: &[String]) -> Result<Self, String> {
        let before = Snapshot::read(settings).map_err(|error| error.to_string())?;
        let parent = root.join("claude-command-history");
        crate::disk::ensure_private_dir(&parent).map_err(|error| error.to_string())?;
        let directory = crate::disk::unique_path(&parent, "command-").map_err(|error| error.to_string())?;
        crate::disk::ensure_private_dir(&directory).map_err(|error| error.to_string())?;
        save(&directory, "before", &before).map_err(|error| error.to_string())?;
        let metadata = serde_json::to_vec_pretty(&serde_json::json!({ "settings": before.path(), "command": command }))
            .map_err(|error| error.to_string())?;
        crate::disk::publish_private(&directory.join("command.json"), &metadata).map_err(|error| error.to_string())?;
        Ok(Self { directory, before })
    }

    pub fn finish(&self) -> Result<Snapshot, String> {
        let after = Snapshot::read(self.before.path()).map_err(|error| error.to_string())?;
        save(&self.directory, "after", &after).map_err(|error| error.to_string())?;
        Ok(after)
    }

    pub fn manifest(&self) -> PathBuf {
        self.directory.join("command.json")
    }
}

fn save(directory: &Path, name: &str, snapshot: &Snapshot) -> io::Result<()> {
    if let Some(bytes) = snapshot.bytes() {
        crate::disk::publish_private(&directory.join(format!("{name}.json")), bytes)?;
    }
    let metadata = snapshot.state.as_ref().map(|state| {
        serde_json::json!({
            "identity": state.identity, "modified": state.modified, "attributes": state.attributes,
        })
    });
    let bytes = serde_json::to_vec_pretty(&metadata)?;
    crate::disk::publish_private(&directory.join(format!("{name}-metadata.json")), &bytes)
}
