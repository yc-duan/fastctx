//! The program the World starts for a login, as it would start it now: read
//! each time the way starting it reads it, from the login's configuration
//! and the search path, and never from what an earlier asking found, since
//! a desktop app moves its program as it upgrades and an install can come
//! or go while no one asks. Only the version is the program's own word,
//! kept from when it was last asked, and it holds while that program's file
//! is the same.

use std::path::Path;

use fastctx_types::agents::ProgramSource;
use fastctx_types::logins::Launcher;

use super::kept::{FileStamp, KeptLogin};
use crate::Node;

/// The program `agent` starts for its login `dir` now; none when there is
/// none to start, or the configuration naming it cannot be read, which
/// starting it would fail on too. `kept` is what the login said when last
/// asked.
pub(super) fn now(node: &Node, agent: &str, dir: &Path, kept: Option<&KeptLogin>) -> Option<Launcher> {
    let (path, desktop_app) = if agent == "codex" {
        let launch = node.codex.launch(dir).ok().flatten()?;
        (launch.binary, launch.source == ProgramSource::DesktopApp)
    } else {
        (crate::claude::program()?, false)
    };
    let version = kept
        .and_then(|login| Some((login.launcher.as_ref()?, login.asked_file.as_ref()?)))
        .filter(|(asked, file)| asked.path == path && FileStamp::of(&path).as_ref() == Some(*file))
        .and_then(|(asked, _)| asked.version.clone());
    Some(Launcher { path, version, desktop_app })
}
