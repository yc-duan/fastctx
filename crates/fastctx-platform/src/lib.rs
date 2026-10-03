//! Each operating-system mechanism the World needs, implemented once.
//!
//! Publishing a file atomically, keeping a directory private, holding a lock,
//! identifying a process across PID reuse, starting a process that outlives
//! its starter: every caller in the World uses the one function here, and
//! the differences between platforms appear in this crate only.

pub mod atomic;
pub mod lock;
pub mod private;

pub use atomic::{Permissions, publish, publish_new};
pub use lock::FileLock;
pub use private::ensure_private_dir;
