//! The shell selects a transport once; every page keeps the same World API.

use std::path::PathBuf;
use std::sync::RwLock;

use fastctx_client::World;
#[cfg(not(target_os = "android"))]
use fastctx_client::device::Device;
use fastctx_platform::{Permissions, ensure_private_dir, publish};
pub use fastctx_types::device::{ConnectionMode as Mode, ConnectionState as State};
#[cfg(desktop)]
use fastctx_types::{ipc::Peer, layout::WorldDir};

pub struct Connection {
    directory: PathBuf,
    current: RwLock<Selected>,
}

struct Selected {
    mode: Mode,
    world: World,
    #[cfg(desktop)]
    program: Option<PathBuf>,
}

impl Connection {
    pub fn open(directory: PathBuf) -> Result<Self, String> {
        ensure_private_dir(&directory)
            .map_err(|error| format!("preparing application directory {}: {error}", directory.display()))?;
        let choice = directory.join("connection.json");
        let device_requested = std::env::args_os().any(|argument| argument == "--device");
        let mode = if device_requested {
            Mode::Device
        } else {
            match std::fs::read(&choice) {
                Ok(bytes) => serde_json::from_slice(&bytes)
                    .map_err(|error| format!("decoding connection choice {}: {error}", choice.display()))?,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    if cfg!(mobile) {
                        Mode::Device
                    } else {
                        Mode::Local
                    }
                }
                Err(error) => return Err(format!("reading connection choice {}: {error}", choice.display())),
            }
        };
        let current = Self::make(&directory, mode)?;
        // A desktop shortcut can select a device before the first page calls
        // World, so a computer used only as an interface never starts a node.
        if device_requested {
            publish(&directory.join("connection.json"), b"\"device\"", Permissions::Private)
                .map_err(|error| error.to_string())?;
        }
        Ok(Self { directory, current: RwLock::new(current) })
    }

    pub fn world(&self) -> World {
        self.current.read().expect("shell connection").world.clone()
    }

    pub fn state(&self) -> State {
        let selected = self.current.read().expect("shell connection");
        State { mode: selected.mode, device: selected.world.device().map(|device| device.state()) }
    }

    pub fn select(&self, mode: Mode) -> Result<(State, bool), String> {
        let mut selected = self.current.write().expect("shell connection");
        let changed = selected.mode != mode;
        if changed {
            if let Some(device) = selected.world.device()
                && device.state().name.is_some()
            {
                return Err("leave this device's network before switching to a local node".into());
            }
            let next = Self::make(&self.directory, mode)?;
            let bytes = serde_json::to_vec(&mode).map_err(|error| error.to_string())?;
            publish(&self.directory.join("connection.json"), &bytes, Permissions::Private)
                .map_err(|error| error.to_string())?;
            *selected = next;
        }
        Ok((State { mode: selected.mode, device: selected.world.device().map(|device| device.state()) }, changed))
    }

    #[cfg(desktop)]
    pub fn local_program(&self) -> Result<PathBuf, String> {
        self.current
            .read()
            .expect("shell connection")
            .program
            .clone()
            .ok_or_else(|| "this interface is a device; there is no local node or agent host".into())
    }

    /// Lets go of what this interface keeps of the conversations it views,
    /// before the World on this computer is removed: from here on nothing
    /// more is written under the fastctx root.
    #[cfg(desktop)]
    pub fn forget_kept(&self) {
        let mut selected = self.current.write().expect("shell connection");
        selected.world = selected.world.clone().without_cache();
    }

    /// Keep the transport check and the snapshot together: switching modes
    /// must never pass a device World to code that installs a node service.
    #[cfg(desktop)]
    pub fn local_world(&self) -> Result<World, String> {
        let selected = self.current.read().expect("shell connection");
        if selected.mode != Mode::Local {
            return Err("this interface is a device; it does not join a local node".into());
        }
        Ok(selected.world.clone())
    }

    fn make(directory: &std::path::Path, mode: Mode) -> Result<Selected, String> {
        #[cfg(target_os = "android")]
        let _ = directory;
        match mode {
            Mode::Device => {
                #[cfg(not(target_os = "android"))]
                let world = {
                    let open = if cfg!(mobile) { Device::open_managed } else { Device::open };
                    let cache = kept(directory)?;
                    let device = open(directory.join("device"), cache.clone()).map_err(|error| error.to_string())?;
                    World::from_device(device).keeping(cache)
                };
                #[cfg(target_os = "android")]
                let world = crate::android::world()?;
                Ok(Selected {
                    mode,
                    world,
                    #[cfg(desktop)]
                    program: None,
                })
            }
            Mode::Local => {
                #[cfg(mobile)]
                return Err("this platform uses a device connection; it cannot run a local node".into());
                #[cfg(desktop)]
                {
                    let root = fastctx_platform::fastctx_root().map_err(|error| error.to_string())?;
                    let program = root.join("bin").join(if cfg!(windows) { "fastctx.exe" } else { "fastctx" });
                    // The node starts from the World program, which on a
                    // computer in no network nothing else may have put there.
                    if !program.is_file() && crate::setup::carried().is_some() {
                        crate::setup::place_initial()
                            .map_err(|error| format!("placing initial program {}: {error}", program.display()))?;
                    }
                    crate::setup::take_carried();
                    // The TUI on this computer keeps what it views in the same place.
                    let cache = fastctx_client::cache::Cache::under(&root)
                        .map_err(|error| format!("opening conversation cache under {}: {error}", root.display()))?;
                    let world = World::new(WorldDir::under(&root), Peer::Command { resident: None, session: None })
                        .starting_node_from(program.clone())
                        .keeping(cache);
                    Ok(Selected { mode, world, program: Some(program) })
                }
            }
        }
    }
}

/// What a device keeps of the conversations it views, in its application's
/// own directory.
pub fn kept(directory: &std::path::Path) -> Result<std::sync::Arc<fastctx_client::cache::Cache>, String> {
    if cfg!(desktop) {
        let root = fastctx_platform::fastctx_root().map_err(|error| error.to_string())?;
        fastctx_client::cache::Cache::under(&root).map_err(|error| error.to_string())
    } else {
        fastctx_client::cache::Cache::open(directory.join("cache").join("viewed")).map_err(|error| error.to_string())
    }
}
