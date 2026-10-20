//! New versions of Fastctx (`world-upgrade.md`, 新版本自己往后怎样升级):
//! what GitHub publishes on each channel, the packages of a release brought
//! here from GitHub or through the network's hub, each checked against the
//! one key every package is signed with, and kept until a newer version
//! replaces them.

pub mod cache;
pub mod catalog;
pub mod download;
pub mod github;
pub mod kernel;
pub mod obtain;
pub mod pace;
pub mod pool;
pub mod state;
pub mod throttle;
pub mod verify;

pub use cache::Cache;
pub use obtain::{Progress, obtain};
