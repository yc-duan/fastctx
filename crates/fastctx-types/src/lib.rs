//! The World's contracts.
//!
//! Every concept that crosses a boundary (a process, a machine, a file on
//! disk) has exactly one type, defined in this crate: names, residents'
//! records, the events every agent's session is translated into, the
//! requests local clients send to the node, and the hands' requests and
//! results. Other crates refer to these types and never redefine them; what
//! is written to disk and what travels on the wire is their serde form.

pub mod access;
pub mod account_records;
pub mod activity_book;
pub mod accounts;
pub mod agent;
pub mod agents;
pub mod attachments;
pub mod authorization;
pub mod calls;
pub mod closest;
pub mod collab;
pub mod collab_prompt;
pub mod collaboration;
pub mod composer;
pub mod contract;
pub mod copy;
pub mod data;
pub mod delivery_control;
pub mod device;
pub mod features;
pub mod file_read;
pub mod file_search;
pub mod files;
pub mod focus;
pub mod goals;
pub mod hub_move;
pub mod ipc;
pub mod kernel;
pub mod layers;
pub mod layout;
pub mod lines;
pub mod logins;
pub mod machine_log;
pub mod management;
pub mod mcp;
pub mod mcp_message;
pub mod memory;
pub mod names;
pub mod node_lifetime;
pub mod observation;
pub mod on;
pub mod pages;
pub mod password;
pub mod plan;
pub mod process;
pub mod profile;
pub mod project;
pub mod remote;
pub mod renewal;
pub mod repair;
pub mod residents;
pub mod run;
pub mod run_facts;
mod scheduling;
pub mod scratch;
pub mod sentinel;
pub mod sentinel_control;
pub mod sentinel_settings;
pub mod sentinel_words;
pub mod session;
pub mod sharing;
pub mod since;
pub mod stats;
pub mod thumbnail;
pub mod timers;
pub mod tolerant;
pub mod town;
pub mod turn_evidence;
pub mod uninstall;
pub mod update;
pub mod usage;
pub mod usage_calls;
pub mod voice;
pub mod watching;
pub mod world;
pub mod write;

pub use calls::{Attach, Call, JournalCall, ProjectCall, Reply};
pub use names::{
    AccountName, Address, AgentName, EntryId, HubName, JobId, JournalName, MachineName, NameError, NameProblem,
    NoticeId, PageName, ResidentId, SentinelId, TownName,
};
pub use process::ProcessIdentity;
pub mod sentinel_notice;
