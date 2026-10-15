//! Native path parsing for local command-line arguments, and the absolute
//! paths the World's file commands take (`world-write.md`, `world-view.md`).

use clap::builder::{PathBufValueParser, TypedValueParser};
use std::path::{Path, PathBuf};

pub fn parser() -> impl TypedValueParser<Value = PathBuf> {
    PathBufValueParser::new().try_map(fastctx_platform::local_path)
}

/// What one of the World's commands says when it refuses what it was
/// given, as it is printed: in its own words, not after `fastctx:`.
#[derive(Debug)]
pub struct Said(pub String);

impl std::fmt::Display for Said {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for Said {}

/// `path` as the World's file commands take it: absolute on this machine.
/// One that is not is refused in the words of `command`, with where this
/// shell is and what the path is from there, which a relative path would
/// have meant, so that it is given again as meant, from the root.
pub fn absolute(command: &str, path: &Path) -> anyhow::Result<PathBuf> {
    let native = fastctx_platform::local_path(path)?;
    if native.is_absolute() {
        return Ok(native);
    }
    let takes = if matches!(command, "edit" | "write") { "an absolute path" } else { "absolute paths" };
    let mut said = format!("{command} {}: {command} takes {takes}.", path.display());
    if let Ok(here) = std::env::current_dir() {
        said.push_str(&format!(" This shell is in {}, so that is {}.", shown(&here), shown(&here.join(&native))));
    }
    Err(Said(said).into())
}

/// A path as the World writes one, with `/` on Windows too.
fn shown(path: &Path) -> String {
    let text = path.display().to_string();
    if cfg!(windows) { text.replace('\\', "/") } else { text }
}
