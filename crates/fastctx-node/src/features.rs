//! The optional parts of the interface this computer shows, and the
//! language it speaks (`fastctx_types::features`). Its graphical interface
//! and its TUI both read them here, so they show the same; each watching
//! interface is told when they change. They only say what an interface
//! shows: nothing the World does depends on them.

use fastctx_types::features::{Feature, Features};
use fastctx_types::ipc::Failure;
use fastctx_types::layout::WorldDir;

/// Who is told when the switches change: the count of changes so far.
#[derive(Default)]
pub(crate) struct Switches {
    changed: tokio::sync::watch::Sender<u64>,
    writing: std::sync::Mutex<()>,
}

impl Switches {
    /// Says when the switches change; what they are is read anew then.
    pub(crate) fn subscribe(&self) -> tokio::sync::watch::Receiver<u64> {
        self.changed.subscribe()
    }
}

/// The switches as this computer keeps them; all off until one is set.
pub(crate) fn read(world: &WorldDir) -> Result<Features, Failure> {
    let path = world.features();
    match std::fs::read(&path) {
        Ok(bytes) => {
            serde_json::from_slice(&bytes).map_err(|error| Failure::new(format!("reading {}: {error}", path.display())))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Features::default()),
        Err(error) => Err(Failure::new(format!("reading {}: {error}", path.display()))),
    }
}

/// Turns `feature` on or off; what the switches are after.
pub(crate) fn set(world: &WorldDir, switches: &Switches, feature: Feature, on: bool) -> Result<Features, Failure> {
    change(world, switches, |features| features.set(feature, on))
}

/// Says the interfaces speak the language tagged `language`.
pub(crate) fn speak(world: &WorldDir, switches: &Switches, language: &str) -> Result<Features, Failure> {
    if let Some(why) = fastctx_types::features::unspoken(language) {
        return Err(Failure::new(why));
    }
    change(world, switches, |features| features.language = Some(language.to_string()))
}

/// Changes the switches as `edit` says, one change at a time, telling the
/// watching interfaces when anything changed.
fn change(world: &WorldDir, switches: &Switches, edit: impl FnOnce(&mut Features)) -> Result<Features, Failure> {
    let _writing = switches.writing.lock().expect("never poisoned");
    let current = read(world)?;
    let mut next = current.clone();
    edit(&mut next);
    if next != current {
        let path = world.features();
        let bytes = serde_json::to_vec_pretty(&next).expect("features serialize");
        fastctx_platform::publish(&path, &bytes, fastctx_platform::Permissions::Private)
            .map_err(|error| Failure::new(format!("writing {}: {error}", path.display())))?;
        switches.changed.send_modify(|count| *count += 1);
    }
    Ok(next)
}
