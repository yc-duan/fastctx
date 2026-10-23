//! Effects supplied by the resident node; no ambient machine or account access.
use fastctx_agent::FutureResult;
use fastctx_types::{MachineName, sentinel::Definition};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReadResult {
    pub text: String,
    pub exit_code: Option<i32>,
    pub full_path: Option<String>,
}
/// Implementations enforce current account authority and a read-only offline sandbox.
pub trait Look: Send + Sync {
    fn look<'a>(
        &'a self,
        definition: &'a Definition,
        machine: &'a MachineName,
        command: &'a str,
    ) -> FutureResult<'a, ReadResult>;
}
pub trait Web: Send + Sync {
    fn fetch<'a>(&'a self, url: &'a str) -> FutureResult<'a, ReadResult>;
    fn search<'a>(&'a self, query: &'a str) -> FutureResult<'a, ReadResult>;
}
/// Only the sentinel's home receives the secret. It is never part of settings replies.
pub trait Credentials: Send + Sync {
    fn model_key<'a>(&'a self, definition: &'a Definition) -> FutureResult<'a, String>;
    fn search_key<'a>(&'a self, definition: &'a Definition) -> FutureResult<'a, String>;
}
