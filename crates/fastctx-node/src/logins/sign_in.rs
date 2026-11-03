//! A login's sign-in files, and reading again who a login is once they
//! change (`world-agent-logins.md`, 世界怎样找到它们). When who a login is
//! is read, how its sign-in files stood is kept with it, by their size and
//! time alone; whenever the logins are looked at, each login whose files no
//! longer stand so is read again (`identify::asked`), the others not at all.
//! A login made or ended in the agent's own program so shows as it is. A
//! sign-in kept in the system's keyring changes no file, and waits for the
//! user's next scan.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use fastctx_types::ipc::Failure;
use fastctx_types::logins::{Found, FoundProgram};

use super::kept::{self, FileStamp, KeptLogin};
use super::{AGENTS, Context, identify};
use crate::Node;

/// How long a look waits for the logins read again; the reading goes on
/// past it and keeps what it finds.
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
    // As Claude Code places them: an empty value is its default directory.
    let secure = match context.env.iter().find(|(name, _)| name.eq_ignore_ascii_case("CLAUDE_SECURESTORAGE_CONFIG_DIR"))
    {
        Some((_, value)) if !value.is_empty() => PathBuf::from(value),
        Some(_) => context.user_home.join(".claude"),
        None => dir.to_path_buf(),
    };
    vec![secure.join(".credentials.json"), dir.join("settings.json")]
}

/// Reads again each login whose sign-in files changed since it was read,
/// waiting `WITHIN` for the answers. Not while a scan runs: it reads them
/// all.
pub(super) async fn ask_changed(node: &Arc<Node>) {
    if node.logins.scanning.load(Ordering::Acquire) {
        return;
    }
    let asking = node.clone();
    let asking = fastctx_platform::task::spawn(async move { again(&asking).await });
    let _ = tokio::time::timeout(WITHIN, asking).await;
}

/// One login to read again: its agent, directory, how the World knows of
/// it, whether it is the agent's default, and how its sign-in files stand.
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
    let ToRead { changed, codex, claude, env } = match fastctx_platform::task::spawn_blocking(move || changed(&finding))
        .await
    {
        Ok(Ok(found)) => found,
        Ok(Err(failure)) => return node.log.line(format_args!("reading changed logins again: {}", failure.message)),
        Err(error) => return node.log.line(format_args!("reading changed logins again: {error}")),
    };
    let mut asking = Vec::new();
    for login in changed {
        node.log.line(format_args!(
            "reading the {} login {} again: its sign-in changed",
            login.agent,
            login.dir.display()
        ));
        let node = node.clone();
        let programs = if login.agent == "codex" { codex.clone() } else { claude.clone() };
        let env = env.clone();
        asking.push(fastctx_platform::task::spawn(async move {
            let asked = identify::asked(&node, login.agent, &login.dir, login.default, programs, env).await;
            (login, asked)
        }));
    }
    let mut answers = Vec::new();
    for asking in asking {
        match asking.await {
            Ok(answer) => answers.push(answer),
            Err(error) => node.log.line(format_args!("reading a login again: {error}")),
        }
    }
    if answers.is_empty() {
        return;
    }
    let keeping = node.clone();
    let kept = fastctx_platform::task::spawn_blocking(move || {
        kept::change(&keeping, |record| {
            for (login, asked) in answers {
                let logins = &mut record.of_mut(login.agent).logins;
                // A scan since may have found it gone.
                if let Some(known) = logins.iter_mut().find(|known| crate::homes::same_dir(&known.dir, &login.dir))
                    && asked.identified.at >= known.identified.at
                {
                    *known = KeptLogin::asked(login.dir, login.found, asked, login.signed, Some(known));
                }
            }
        })
    })
    .await;
    match kept {
        Ok(Ok(())) => {}
        Ok(Err(error)) => node.log.line(format_args!("keeping the logins read again: {error}")),
        Err(error) => node.log.line(format_args!("keeping the logins read again: {error}")),
    }
}

/// What reading changed logins again needs: the logins read before whose
/// sign-in files have changed since, each agent's programs found here,
/// whose records say their versions, and the user's environment.
struct ToRead {
    changed: Vec<Changed>,
    codex: Vec<FoundProgram>,
    claude: Vec<FoundProgram>,
    env: Vec<(std::ffi::OsString, std::ffi::OsString)>,
}

fn changed(node: &Node) -> Result<ToRead, Failure> {
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
    Ok(ToRead {
        changed,
        codex: record.codex.programs.clone(),
        claude: record.claude.programs.clone(),
        env: context.env,
    })
}
