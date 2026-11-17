//! Current service tables, separate from the frozen legacy migration inventory.

#[derive(Clone, Copy, Debug)]
pub struct Table {
    pub name: &'static str,
    pub kind: &'static str,
    pub owner: &'static str,
}

include!(concat!(env!("OUT_DIR"), "/runtime_tables.rs"));
