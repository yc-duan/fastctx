//! An inactive shadow schema requiring explicit installation by a service.
//! Importing this module neither opens a database nor claims a world format.

#[path = "schema_tables.rs"]
mod tables;
pub use tables::TABLES;

pub const COMPONENT: &str = "sync.shadow";
pub const VERSION: u32 = 1;
pub const SCHEMA: &str = include_str!("schema.sql");
