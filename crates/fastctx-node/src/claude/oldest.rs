//! Sessions start only in a Claude Code the World works with, including one
//! connected before the World checked when connecting, or put back to an
//! older version since. Each program file is asked its version once; a file
//! that changed is asked again.

use std::path::PathBuf;
use std::sync::Mutex;

use fastctx_types::agents::Fact;
use fastctx_types::ipc::Failure;

use crate::Node;
use crate::logins::kept::FileStamp;

/// The program last found new enough, as its file stood then.
#[derive(Default)]
pub(crate) struct Checked(Mutex<Option<(PathBuf, FileStamp)>>);

pub(super) async fn check(node: &Node, command: &super::Command) -> Result<(), Failure> {
    let stamp = FileStamp::of(&command.program);
    let known = |checked: &Option<(PathBuf, FileStamp)>| {
        checked.as_ref().is_some_and(|(path, kept)| *path == command.program && Some(kept) == stamp.as_ref())
    };
    if known(&node.claude.checked.0.lock().expect("never poisoned")) {
        return Ok(());
    }
    let super::Command { program, cwd, env } = command.clone();
    let said = tokio::task::spawn_blocking(move || fastctx_hosts::agents::version(&program, &cwd, &env))
        .await
        .map_err(|error| Failure::new(format!("asking Claude Code its version: {error}")))?;
    // A program that cannot say its version fails to start a session as
    // well, in its own words.
    let Fact::Known { value } = said else { return Ok(()) };
    fastctx_hosts::claude::supported(&value).map_err(Failure::new)?;
    if let Some(stamp) = stamp {
        *node.claude.checked.0.lock().expect("never poisoned") = Some((command.program.clone(), stamp));
    }
    Ok(())
}
