//! Each operating-system mechanism the World needs, implemented once.
//!
//! Publishing a file atomically, keeping a directory private, holding a lock,
//! identifying a process across PID reuse, starting a process that outlives
//! its starter, the endpoint where the node listens: every caller in the
//! World uses the one implementation here, and the differences between
//! platforms appear in this crate only.

pub mod atomic;
#[cfg(windows)]
mod command_line;
pub mod detach;
pub mod home;
pub mod identity;
pub mod ipc;
pub mod lock;
pub mod private;
#[cfg(target_os = "linux")]
pub mod procfs;
#[cfg(target_os = "linux")]
pub mod tree;
#[cfg(unix)]
mod unix_fds;
#[cfg(windows)]
mod windows_user;

pub use atomic::{Permissions, publish, publish_new};
pub use detach::{DetachedProcess, spawn_detached};
pub use home::{fastctx_root, user_home};
pub use identity::ProcessIdentity;
pub use lock::FileLock;
pub use private::ensure_private_dir;
#[cfg(target_os = "linux")]
pub use tree::ProcessTree;
