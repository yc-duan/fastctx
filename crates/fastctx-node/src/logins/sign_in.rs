//! A login's sign-in files, and reading again who a login is once they
//! change (`world-agent-logins.md`, 世界怎样找到它们). When who a login is
//! is read, how its sign-in files stood is kept with it, by their size and
//! time alone; a background pass reads each login whose files no longer
//! stand so again (`identify::asked`), the others not at all.
//! A login made or ended in the agent's own program so shows as it is. A
//! sign-in kept in the system's keyring changes no file, and waits for the
//! user's next scan.
//!
//! Files written again are not a new sign-in. What a sign-in is, apart from
//! how its files stand, is kept as a digest of the credential itself
//! (`credential`): a login whose settings were edited around the same token
//! is asked again and stays the account it was, with the limits read for it.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;

use fastctx_types::ipc::Failure;
use fastctx_types::logins::{Found, FoundProgram};
use sha2::{Digest, Sha256};

use super::kept::{self, FileStamp, KeptLogin};
use super::{AGENTS, Context, identify};
use crate::Node;

/// How the sign-in files of `agent`'s login `dir` stand now.
pub(super) fn now(context: &Context, agent: &str, dir: &Path) -> Vec<Option<FileStamp>> {
    files(context, agent, dir).iter().map(|file| FileStamp::of(file)).collect()
}

/// The files the sign-in of `agent`'s login `dir` lives in, as the agent
/// keeps it unless told to use the system's keyring: Codex's `auth.json`;
/// Claude Code's `.credentials.json`, in `CLAUDE_SECURESTORAGE_CONFIG_DIR`
/// when the user's environment names one, and its `settings.json`, where a
/// long-lived token is put.
pub(super) fn files(context: &Context, agent: &str, dir: &Path) -> Vec<PathBuf> {
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

/// The settings a Claude Code login's sign-in can live in besides its
/// credentials file: a long-lived token or a key, the program asked for
/// one, and which provider answers them.
const CLAUDE_SIGN_IN_ENV: [&str; 7] = [
    "CLAUDE_CODE_OAUTH_TOKEN",
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_BASE_URL",
    "CLAUDE_CODE_USE_BEDROCK",
    "CLAUDE_CODE_USE_VERTEX",
    "CLAUDE_CODE_USE_FOUNDRY",
];

/// What the sign-in of `agent`'s login `dir` is, apart from how its files
/// stand: a digest of the credentials in them, from which they cannot be
/// told. Of Claude Code's settings only what signs in counts, so the same
/// token among other settings is the same sign-in. None where the files
/// hold no credential that can be read.
pub(super) fn credential(context: &Context, agent: &str, dir: &Path) -> Option<String> {
    fastctx_platform::read_budget::within(super::read_limits::DIGEST, || credential_in(context, agent, dir))
        .ok()
        .flatten()
}

fn credential_in(context: &Context, agent: &str, dir: &Path) -> Option<String> {
    let mut digest = Sha256::new();
    let mut any = false;
    for (index, file) in files(context, agent, dir).iter().enumerate() {
        let Ok(bytes) = fastctx_platform::disk::read(file) else { continue };
        let part = if file.file_name().is_some_and(|name| name == "settings.json") {
            settings_sign_in(&bytes)
        } else {
            Some(bytes)
        };
        let Some(part) = part else { continue };
        digest.update([index as u8]);
        digest.update((part.len() as u64).to_le_bytes());
        digest.update(&part);
        any = true;
    }
    any.then(|| format!("{:x}", digest.finalize()))
}

/// What of a Claude Code settings file signs in, in a fixed order; the
/// whole file where it cannot be read as settings.
fn settings_sign_in(bytes: &[u8]) -> Option<Vec<u8>> {
    let Ok(serde_json::Value::Object(settings)) = serde_json::from_slice(bytes) else {
        return Some(bytes.to_vec());
    };
    let env = settings.get("env").and_then(serde_json::Value::as_object);
    let mut found: Vec<(&str, &serde_json::Value)> =
        CLAUDE_SIGN_IN_ENV.iter().filter_map(|name| Some((*name, env?.get(*name)?))).collect();
    if let Some(helper) = settings.get("apiKeyHelper") {
        found.push(("apiKeyHelper", helper));
    }
    if found.is_empty() {
        return None;
    }
    Some(serde_json::to_vec(&found).expect("JSON values serialize"))
}

/// One login to read again: its agent, directory, how the World knows of
/// it, whether it is the agent's default, and how its sign-in files stand.
struct Changed {
    agent: &'static str,
    dir: PathBuf,
    found: Found,
    default: bool,
    signed: Vec<Option<FileStamp>>,
    credential: Option<String>,
    identified_at: jiff::Timestamp,
}

pub(super) async fn refresh(node: &Arc<Node>) -> crate::passes::Left {
    use crate::passes::Left;
    if node.logins.scanning.load(Ordering::Acquire) {
        return Left::Work;
    }
    // Identification stays separate from readers and from a scan.
    let _asking = node.logins.asking.lock().await;
    let finding = node.clone();
    let ToRead { changed, unmarked, codex, claude, env } =
        match fastctx_platform::task::spawn_blocking(move || changed(&finding)).await {
            Ok(Ok(found)) => found,
            Ok(Err(failure)) => {
                node.log.line(format_args!("reading changed logins again: {}", failure.message));
                return Left::Work;
            }
            Err(error) => {
                node.log.line(format_args!("reading changed logins again: {error}"));
                return Left::Work;
            }
        };
    let mut unfinished = false;
    if !unmarked.is_empty() {
        let keeping = node.clone();
        let marked = fastctx_platform::task::spawn_blocking(move || {
            kept::change(&keeping, |record| {
                for login in unmarked {
                    let logins = &mut record.of_mut(login.agent).logins;
                    if let Some(known) = logins.iter_mut().find(|known| crate::homes::same_dir(&known.dir, &login.dir))
                        && known.credential.is_none()
                        && known.signed.as_ref() == Some(&login.signed)
                    {
                        known.sign(login.signed, Some(login.credential));
                    }
                }
            })
        })
        .await;
        match marked {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                node.log.line(format_args!("keeping the logins' sign-ins: {error}"));
                unfinished = true;
            }
            Err(error) => {
                node.log.line(format_args!("keeping the logins' sign-ins: {error}"));
                unfinished = true;
            }
        }
    }
    // Bound each batch and let older readings go first, so a backlog advances.
    let more = changed.len() > 8;
    let mut changed = changed;
    changed.sort_by_key(|login| login.identified_at);
    let mut asking = tokio::task::JoinSet::new();
    for login in changed.into_iter().take(8) {
        node.log.line(format_args!(
            "reading the {} login {} again: its sign-in changed",
            login.agent,
            login.dir.display()
        ));
        let node = node.clone();
        let programs = if login.agent == "codex" { codex.clone() } else { claude.clone() };
        let env = env.clone();
        asking.spawn(async move {
            let asked = identify::asked(&node, login.agent, &login.dir, login.default, programs, env).await;
            (login, asked)
        });
    }
    let mut answers = Vec::new();
    while let Some(answer) = asking.join_next().await {
        match answer {
            Ok(answer) => answers.push(answer),
            Err(error) => {
                node.log.line(format_args!("reading a login again: {error}"));
                unfinished = true;
            }
        }
    }
    if answers.is_empty() {
        return if more || unfinished { Left::Work } else { Left::Nothing };
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
                    *known =
                        KeptLogin::asked(login.dir, login.found, asked, login.signed, login.credential, Some(known));
                }
            }
        })
    })
    .await;
    match kept {
        Ok(Ok(())) => {
            node.shown_changed();
            if more {
                super::sign_in_refresh::ask(node);
            }
            if unfinished { Left::Work } else { Left::Nothing }
        }
        Ok(Err(error)) => {
            node.log.line(format_args!("keeping the logins read again: {error}"));
            Left::Work
        }
        Err(error) => {
            node.log.line(format_args!("keeping the logins read again: {error}"));
            Left::Work
        }
    }
}

/// A login whose sign-in files stand as they did when it was asked, with
/// what its sign-in is, which was not kept then.
struct Unmarked {
    agent: &'static str,
    dir: PathBuf,
    signed: Vec<Option<FileStamp>>,
    credential: String,
}

/// What reading changed logins again needs: the logins read before whose
/// sign-in files have changed since, those whose files stand as they did
/// while what their sign-in is was never kept, each agent's programs found
/// here, whose records say their versions, and the user's environment.
struct ToRead {
    changed: Vec<Changed>,
    unmarked: Vec<Unmarked>,
    codex: Vec<FoundProgram>,
    claude: Vec<FoundProgram>,
    env: Vec<(std::ffi::OsString, std::ffi::OsString)>,
}

fn changed(node: &Node) -> Result<ToRead, Failure> {
    let context = Context::of(node)?;
    super::workbench::seed(node, &context);
    let record = kept::read(node).map_err(|error| Failure::new(format!("reading the agents' logins: {error}")))?;
    let mut changed = Vec::new();
    let mut unmarked = Vec::new();
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
                    credential: credential(&context, agent, &login.dir),
                    signed,
                    identified_at: login.identified.at,
                });
            } else if login.credential.is_none()
                && let Some(credential) = credential(&context, agent, &login.dir)
            {
                unmarked.push(Unmarked { agent, dir: login.dir.clone(), signed, credential });
            }
        }
    }
    Ok(ToRead {
        changed,
        unmarked,
        codex: record.codex.programs.clone(),
        claude: record.claude.programs.clone(),
        env: context.env,
    })
}
