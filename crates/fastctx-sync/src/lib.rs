//! Registered data and durable synchronization boundaries.
//!
//! The contracts are independent of a node, a hub and a mobile application.
//! Storage implementations own their databases; consumers receive typed handles.

pub mod catalog;
pub mod cursor;
pub mod identity;
pub mod manifest;
pub mod metrics;
pub mod registry;
pub mod scheduler;
pub mod store;
pub mod transfer;
