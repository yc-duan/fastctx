//! The World's contracts.
//!
//! Every concept that crosses a boundary (a process, a machine, a file on
//! disk) has exactly one type, defined in this crate: names, residents'
//! records, the events every agent's session is translated into, the
//! requests local clients send to the node, and the hands' requests and
//! results. Other crates refer to these types and never redefine them; what
//! is written to disk and what travels on the wire is their serde form.

pub mod names;

pub use names::{Address, AgentName, EntryId, JobId, MachineName, NameError, ResidentId, TownName};
