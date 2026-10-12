//! Everything the World knows of each agent host, and nowhere else: how it
//! starts a session, hands it words and hears what it does.
//!
//! Each host is reached only through what it publishes for programs to use
//! (`world.md`, 接入宿主只靠它们写明的接口): Codex through its app-server,
//! Claude Code through its plugin and mod. Whatever a host reports comes
//! out in the World's own terms, `fastctx_types::session`.

pub mod agents;
pub mod claude;
pub mod codex;
pub mod image_digest;
pub mod image_record;
pub mod inventory;
pub mod world_tools;
