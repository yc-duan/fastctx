//! Claude semantics over a completely validated, disk-backed JSON value. Large
//! content remains a span; consumers never reconstruct the host's whole Value.
pub mod block;
pub mod children;
pub mod fields;
pub mod prompt;
pub mod row;
