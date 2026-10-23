//! A small, bounded agent runtime independent of the node and its hosts.
pub mod model;
pub mod runtime;
pub mod state;
pub mod words;

use std::{future::Future, pin::Pin};
pub type FutureResult<'a, T> = Pin<Box<dyn Future<Output = Result<T, String>> + Send + 'a>>;
pub use runtime::{Application, Decision, Limits, Run, ToolOutput, run};
