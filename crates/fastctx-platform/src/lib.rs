//! Each operating-system mechanism the World needs, implemented once.
//!
//! Publishing a file atomically, keeping a directory private, holding a lock,
//! identifying a process across PID reuse, starting a process that outlives
//! its starter, the endpoint where the node listens: every caller in the
//! World uses the one implementation here, and the differences between
//! platforms appear in this crate only.

pub mod atomic;
pub mod detach;
pub mod environment;
pub mod home;
pub mod hostname;
pub mod identity;
pub mod ipc;
pub mod legacy_text;
pub mod lock;
pub mod private;
#[cfg(target_os = "linux")]
pub mod procfs;
#[cfg(windows)]
pub mod registry;
#[cfg(any(target_os = "linux", windows))]
pub mod tree;
#[cfg(unix)]
mod unix_fds;
#[cfg(windows)]
mod windows_process;
#[cfg(windows)]
mod windows_user;

pub use atomic::{Permissions, publish, publish_new};
pub use detach::{DetachedProcess, spawn_detached};
pub use environment::{in_job, user_environment, world_process_environment};
pub use home::{fastctx_root, user_home};
pub use hostname::hostname;
pub use identity::{current_identity, identity_of, is_alive};
pub use legacy_text::decode_legacy;
pub use lock::FileLock;
pub use private::ensure_private_dir;
#[cfg(any(target_os = "linux", windows))]
pub use tree::ProcessTree;
