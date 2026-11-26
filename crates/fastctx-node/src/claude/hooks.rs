//! Whether Claude Code runs the World's hooks here
//! (`fastctx_types::world::HooksSkipped`): read from the start of each World
//! session's own record until it tells, by a notice that a hook was passed
//! over or a reply of the model's past which none comes for its first turn;
//! and told in this machine's facts while the latest session that told said
//! so, for `fastctx nodes` and the interfaces to say what to add to the
//! machine's managed settings. Claude Code 2.1.322 seats a plugin of its own
//! before every user plugin where a machine has managed settings, and it
//! sent the World's hooks on past (`ops-production`, 11-10 to 11-25).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};

use fastctx_hosts::claude::{exchange, projects};
use fastctx_types::world::HooksSkipped;

use crate::Node;

/// The World's plugin, as Claude Code names it in its notices.
const PLUGIN: &str = "fastctx-world";

/// The sessions whose records told, and those being read now.
static TOLD: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Default::default);
static READING: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Default::default);
/// What the latest session that told said, where its hooks were skipped.
static SKIPPED: LazyLock<Mutex<Option<HooksSkipped>>> = LazyLock::new(Default::default);
/// When the latest session here that ran them began: one that skipped them,
/// begun before it and told after it, says nothing of now.
static RAN: LazyLock<Mutex<Option<jiff::Timestamp>>> = LazyLock::new(Default::default);

/// The World's hooks Claude Code skips here, as the latest session that
/// told said; none while they run.
pub(crate) fn skipped() -> Option<HooksSkipped> {
    SKIPPED.lock().expect("never poisoned").clone()
}

/// Reads, in the background, what the start of `session`'s record says of
/// the World's hooks, until it tells: each report of the session asks again
/// while it has not. `home` is the Claude Code login it runs in.
pub(crate) fn look(node: &Arc<Node>, session: &str, directory: &Path, home: Option<PathBuf>) {
    if TOLD.lock().expect("never poisoned").contains(session)
        || !READING.lock().expect("never poisoned").insert(session.to_string())
    {
        return;
    }
    let (node, session, directory) = (node.clone(), session.to_string(), directory.to_path_buf());
    fastctx_platform::task::spawn(async move {
        let reading = session.clone();
        let read = fastctx_platform::task::spawn_blocking(move || -> std::io::Result<_> {
            let Some(registration) = exchange::registration(&directory)? else { return Ok(None) };
            let path = match exchange::transcript(&directory)? {
                Some(path) => Some(path),
                None => match &home {
                    Some(home) => projects::record(home, registration.cwd.as_deref().map(Path::new), &reading)?,
                    None => None,
                },
            };
            let Some(path) = path else { return Ok(None) };
            Ok(Some((projects::hooks_seen(&path, PLUGIN)?, registration.at)))
        })
        .await;
        READING.lock().expect("never poisoned").remove(&session);
        let Ok(Ok(Some((seen, began)))) = read else { return };
        if seen.skipped.is_empty() && !seen.answered {
            return;
        }
        {
            let mut told = TOLD.lock().expect("never poisoned");
            // A long-running node lets go of what it told long ago.
            if told.len() >= 16_384 {
                told.clear();
            }
            told.insert(session.clone());
        }
        let changed = {
            let mut skipped = SKIPPED.lock().expect("never poisoned");
            let mut ran = RAN.lock().expect("never poisoned");
            let before = skipped.clone();
            if !seen.skipped.is_empty() {
                let since_ran = match (*ran, began) {
                    (Some(ran), Some(began)) => began > ran,
                    (Some(_), None) => false,
                    (None, _) => true,
                };
                if since_ran && skipped.as_ref().is_none_or(|kept| kept.at <= began) {
                    *skipped = Some(HooksSkipped {
                        by: seen.by.unwrap_or_else(|| "something that runs before them".to_string()),
                        hooks: seen.skipped,
                        session: session.clone(),
                        at: began,
                        plugins: node.world.path().join("claude-plugins").to_string_lossy().replace('\\', "/"),
                    });
                }
            } else {
                if let Some(began) = began {
                    *ran = Some(ran.map_or(began, |ran| ran.max(began)));
                }
                if skipped.as_ref().is_some_and(|kept| kept.at < began) {
                    // A session begun since ran them: what was missing is there now.
                    *skipped = None;
                }
            }
            (*skipped != before).then(|| skipped.clone())
        };
        if let Some(now) = changed {
            match now {
                Some(skipped) => node.log.line(format_args!(
                    "Claude Code skips the World's hooks here ({}): \"{}\" runs first; {}",
                    skipped.hooks.join(", "),
                    skipped.by,
                    skipped.remedy()
                )),
                None => node.log.line("Claude Code runs the World's hooks here again"),
            }
            node.shown_changed();
        }
    });
}
