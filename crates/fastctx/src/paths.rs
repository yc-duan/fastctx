//! Native path parsing for local command-line arguments.

use clap::builder::{PathBufValueParser, TypedValueParser};
use std::path::PathBuf;

pub fn parser() -> impl TypedValueParser<Value = PathBuf> {
    PathBufValueParser::new().try_map(fastctx_platform::local_path)
}
