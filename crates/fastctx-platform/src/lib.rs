//! Each operating-system mechanism the World needs, implemented once.
//!
//! Publishing a file atomically, keeping a directory private, holding a lock,
//! identifying a process across PID reuse, starting a process that outlives
//! its starter, the endpoint where the node listens: every caller in the
//! World uses the one implementation here, and the differences between
//! platforms appear in this crate only.

pub mod atomic;
pub mod awake;
pub mod bounded;
#[cfg(any(target_os = "linux", target_os = "macos", windows))]
pub mod child;
pub mod cpu_share;
#[cfg(target_os = "macos")]
pub mod darwin_proc;
pub mod detach;
pub mod directory_alias;
pub mod disk_usage;
pub mod disks;
pub mod doing;
pub mod elevation;
pub mod environment;
pub mod event_queue;
pub mod file_read;
pub mod file_version;
pub mod form;
pub mod home;
pub mod host_process;
pub mod hostname;
pub mod identity;
pub mod image_usage;
pub mod ipc;
pub mod job_membership;
pub mod legacy_text;
pub mod local_path;
pub mod lock;
pub mod memory;
#[cfg(target_os = "linux")]
pub mod mountinfo;
#[cfg(any(target_os = "linux", target_os = "macos", windows))]
pub mod output_pipe;
pub mod owned_directory;
#[cfg(any(target_os = "linux", target_os = "macos", windows))]
pub mod owned_images;
pub mod parent;
pub mod privacy;
pub mod private;
#[cfg(target_os = "linux")]
pub mod procfs;
#[cfg(windows)]
pub mod protected_directory;
#[cfg(windows)]
pub mod registry;
pub mod rolling_log;
pub mod service_channel;
#[cfg(windows)]
pub mod shell_process;
pub mod shell_quote;
pub mod system;
pub mod system_home;
#[cfg(any(target_os = "linux", target_os = "macos", windows))]
pub mod terminate;
#[cfg(any(target_os = "linux", target_os = "macos", windows))]
pub mod tree;
#[cfg(any(target_os = "linux", target_os = "macos", windows))]
pub mod uninstall_gate;
#[cfg(unix)]
pub mod unix_account;
#[cfg(unix)]
pub mod unix_executable;
#[cfg(unix)]
mod unix_fds;
pub mod unix_socket;
pub mod viewed_cache;
#[cfg(windows)]
mod windows_job;
#[cfg(windows)]
mod windows_process;
#[cfg(windows)]
mod windows_user;

pub use atomic::{Permissions, PublishFailure, Staged, publish, publish_checked, publish_new};
#[cfg(any(target_os = "linux", target_os = "macos", windows))]
pub use child::Child;
pub use detach::{DetachedProcess, spawn_detached};
pub use environment::{in_job, user_environment, world_process_environment};
pub use home::{RELEASED, fastctx_root, fastctx_root_in, follows_releases, temp_dir, user_home, user_id};
pub use hostname::hostname;
pub use identity::{current_identity, has_ended, identity_of, is_alive};
pub use legacy_text::decode_legacy;
pub use local_path::local_path;
pub use lock::FileLock;
pub use private::ensure_private_dir;
#[cfg(any(target_os = "linux", target_os = "macos", windows))]
pub use tree::ProcessTree;
#[cfg(windows)]
pub use windows_user::{account_name, current_user_sid};
