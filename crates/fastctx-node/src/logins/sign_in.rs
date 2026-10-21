//! A login's sign-in files, and asking a login again once they change
//! (`world-agent-logins.md`, 世界怎样找到它们). When a login is asked who
//! it is, how its sign-in files stood is kept with the answer, by their size
//! and time alone, never what is in them; whenever the logins are looked
//! at, each login whose files no longer stand so is asked again, the others
//! not at all. A login made or ended in the agent's own program so shows as
//! it is, with nothing started while nothing changes. A sign-in kept in the
//! system's keyring changes no file, and waits for the user's next scan.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use fastctx_types::ipc::Failure;
use fastctx_types::logins::Found;

use super::kept::{self, FileStamp, KeptLogin};
use super::{AGENTS, Context, identify};
use crate::Node;

/// How long a look waits for the logins asked again; the asking goes on
/// past it and keeps what it hears.
const WITHIN: Duration = Duration::from_secs(4);

/// How the sign-in files of `agent`'s login `dir` stand now.
pub(super) fn now(context: &Context, agent: &str, dir: &Path) -> Vec<Option<FileStamp>> {
    files(context, agent, dir).iter().map(|file| FileStamp::of(file)).collect()
}

/// The files the sign-in of `agent`'s login `dir` lives in, as the agent
/// keeps it unless told to use the system's keyring: Codex's `auth.json`;
/// Claude Code's `.credentials.json`, in `CLAUDE_SECURESTORAGE_CONFIG_DIR`
/// when the user's environment names one, and its `settings.json`, where a
/// long-lived token is put.
fn files(context: &Context, agent: &str, dir: &Path) -> Vec<PathBuf> {
    if agent == "codex" {
        return vec![dir.join("auth.json")];
    }
    let secure = context
        .env
        .iter()
        .find(|(name, value)| name.eq_ignore_ascii_case("CLAUDE_SECURESTORAGE_CONFIG_DIR") && !value.is_empty())
        .map(|(_, value)| PathBuf::from(value))
        .unwrap_or_else(|| dir.to_path_buf());
    vec![secure.join(".credentials.json"), dir.join("settings.json")]
}

/// Asks again each login whose sign-in files changed since it was asked,
/// waiting `WITHIN` for the answers. Not while a scan runs: it asks them all.
pub(super) async fn ask_changed(node: &Arc<Node>) {
    if node.logins.scanning.load(Ordering::Acquire) {
        return;
    }
    let asking = node.clone();
    let asking = tokio::spawn(async move { again(&asking).await });
    let _ = tokio::time::timeout(WITHIN, asking).await;
}

/// One login to ask again: its agent, directory, how the World knows of it,
/// whether it is the agent's default, and how its sign-in files stand.
struct Changed {
    agent: &'static str,
    dir: PathBuf,
    found: Found,
    default: bool,
    signed: Vec<Option<FileStamp>>,
}

async fn again(node: &Arc<Node>) {
    // One round at a time: a look while another asks waits for it, and then
    // finds nothing changed.
    let _asking = node.logins.asking.lock().await;
    let finding = node.clone();
    let (changed, claude_programs) = match tokio::task::spawn_blocking(move || changed(&finding)).await {
        Ok(Ok(found)) => found,
        Ok(Err(failure)) => return node.log.line(format_args!("asking changed logins again: {}", failure.message)),
        Err(error) => return node.log.line(format_args!("asking changed logins again: {error}")),
    };
    let mut asking = Vec::new();
    for login in changed {
        node.log.line(format_args!(
            "asking the {} login {} again: its sign-in changed",
            login.agent,
            login.dir.display()
        ));
        let node = node.clone();
        let programs = claude_programs.clone();
        asking.push(tokio::spawn(async move {
            let asked = if login.agent == "codex" {
                identify::codex(&node, &login.dir, false).await
            } else {
                let (dir, default) = (login.dir.clone(), login.default);
                match tokio::task::spawn_blocking(move || identify::claude(&dir, default, &programs)).await {
                    Ok(Ok(asked)) => asked,
                    Ok(Err(failure)) => identify::failed(jiff::Timestamp::now(), failure.message),
                    Err(error) => identify::failed(jiff::Timestamp::now(), error.to_string()),
                }
            };
            (login, asked)
        }));
    }
    let mut answers = Vec::new();
    for asking in asking {
        match asking.await {
            Ok(answer) => answers.push(answer),
            Err(error) => node.log.line(format_args!("asking a login again: {error}")),
        }
    }
    if answers.is_empty() {
        return;
    }
    let keeping = node.clone();
    let kept = tokio::task::spawn_blocking(move || {
        kept::change(&keeping, |record| {
            for (login, asked) in answers {
                let logins = &mut record.of_mut(login.agent).logins;
                // A scan since may have found it gone.
                if let Some(known) = logins.iter_mut().find(|known| crate::homes::same_dir(&known.dir, &login.dir))
                    && asked.identified.at >= known.identified.at
                {
                    *known = KeptLogin::asked(login.dir, login.found, asked, login.signed);
                }
            }
        })
    })
    .await;
    match kept {
        Ok(Ok(())) => {}
        Ok(Err(error)) => node.log.line(format_args!("keeping the logins asked again: {error}")),
        Err(error) => node.log.line(format_args!("keeping the logins asked again: {error}")),
    }
}

/// The logins asked before whose sign-in files have changed since, and the
/// Claude programs found here, whose records say Claude Code's version.
fn changed(node: &Node) -> Result<(Vec<Changed>, Vec<fastctx_types::logins::FoundProgram>), Failure> {
    let context = Context::of(node)?;
    let record = kept::read(node).map_err(|error| Failure::new(format!("reading the agents' logins: {error}")))?;
    let mut changed = Vec::new();
    for agent in AGENTS {
        for login in &record.of(agent).logins {
            // Kept before sign-ins were: asked again only by the next asking.
            let Some(kept) = &login.signed else { continue };
            let signed = now(&context, agent, &login.dir);
            if *kept != signed {
                changed.push(Changed {
                    agent,
                    dir: login.dir.clone(),
                    found: login.found,
                    default: crate::homes::same_dir(&login.dir, context.default_of(agent)),
                    signed,
                });
            }
        }
    }
    Ok((changed, record.claude.programs.clone()))
}
