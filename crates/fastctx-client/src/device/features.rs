//! The interface's optional parts a device without a node shows
//! (`fastctx_types::features`): a phone, or a computer used only as a
//! device, keeps its own beside its identity, and tells its interface when
//! they change.

use std::path::Path;

use fastctx_types::features::{Feature, Features};

use super::{Device, failed};
use crate::Error;

/// Who is told when the switches change: the count of changes so far.
#[derive(Default)]
pub(super) struct Switches {
    changed: tokio::sync::watch::Sender<u64>,
    writing: std::sync::Mutex<()>,
}

fn path(directory: &Path) -> std::path::PathBuf {
    fastctx_types::layout::WorldDir::at(directory).features()
}

impl Device {
    /// The switches as this device keeps them; all off until one is set.
    pub(crate) fn features(&self) -> Result<Features, Error> {
        let path = path(self.directory());
        match std::fs::read(&path) {
            Ok(bytes) => {
                serde_json::from_slice(&bytes).map_err(|error| failed(format!("reading {}: {error}", path.display())))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Features::default()),
            Err(error) => Err(failed(format!("reading {}: {error}", path.display()))),
        }
    }

    /// Turns `feature` on or off; what the switches are after.
    pub(crate) fn set_feature(&self, feature: Feature, on: bool) -> Result<Features, Error> {
        self.change(|features| features.set(feature, on))
    }

    /// Says this device's interface speaks the language tagged `language`.
    pub(crate) fn set_language(&self, language: &str) -> Result<Features, Error> {
        if let Some(why) = fastctx_types::features::unspoken(language) {
            return Err(failed(why));
        }
        self.change(|features| features.language = Some(language.to_string()))
    }

    fn change(&self, edit: impl FnOnce(&mut Features)) -> Result<Features, Error> {
        let _writing = self.features.writing.lock().expect("never poisoned");
        let current = self.features()?;
        let mut next = current.clone();
        edit(&mut next);
        if next != current {
            let path = path(self.directory());
            let bytes = serde_json::to_vec_pretty(&next).expect("features serialize");
            fastctx_platform::publish(&path, &bytes, fastctx_platform::Permissions::Private)
                .map_err(|error| failed(format!("writing {}: {error}", path.display())))?;
            self.features.changed.send_modify(|count| *count += 1);
        }
        Ok(next)
    }

    /// Says when the switches change.
    pub(crate) fn feature_changes(&self) -> tokio::sync::watch::Receiver<u64> {
        self.features.changed.subscribe()
    }
}
