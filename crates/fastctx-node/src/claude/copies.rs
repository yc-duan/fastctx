//! Durable exclusions and retryable cleanup for unwanted Claude session copies.

use crate::Node;
use fastctx_hosts::claude::background;
use fastctx_jobs::{Kind, Outcome, Taken};
use fastctx_storage::{Priority, rusqlite::OptionalExtension};
use fastctx_types::ipc::Failure;
use serde::{Deserialize, Serialize};
use std::{
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
    time::Duration,
};

const KIND: &str = "claude/discard-copy";
/// How long a copy Claude Code never lists is looked for before its cleanup
/// is done with: it stays excluded, and nothing is left to stop.
const NEVER_LISTED: i64 = 600;
/// How long after a wake began a background session that names no
/// conversation is taken for the copy started in its place.
const AFTER_A_WAKE: i64 = 600;

#[derive(Default)]
pub(crate) struct Copies {
    cleaning: crate::leases::Registry<String, tokio::sync::Mutex<()>>,
}

impl Copies {
    pub(crate) fn is_copy(&self, node: &Node, session: &str) -> Result<bool, Failure> {
        let short = session.get(..8).unwrap_or(session);
        node.database
            .read_now(|db| {
                Ok(db.query_row("SELECT EXISTS(SELECT 1 FROM claude_copies WHERE short=?1)", [short], |row| {
                    row.get(0)
                })?)
            })
            .map_err(failure)
    }

    /// Whether the background session `session`, which names no conversation
    /// and is no conversation's, is to be kept out: it came while a wake was
    /// in flight here or shortly after one, when Claude Code starts a copy in
    /// place of the session it was asked to wake. It is excluded for good,
    /// so that it is kept out when it asks again later, and is not stopped:
    /// it may be a session its user started then. A session that comes at
    /// any other time is the user's own, and becomes a conversation as before.
    pub(crate) fn unwanted(&self, node: &Node, session: &str) -> Result<bool, Failure> {
        let now = jiff::Timestamp::now().as_second();
        if !super::wake_store::lately(node, now - AFTER_A_WAKE)? {
            return Ok(false);
        }
        let short = session.get(..8).unwrap_or(session).to_string();
        let copy = Copy {
            at: now,
            home: None,
            short: short.clone(),
            removed: true,
            seen: false,
            problem: Some("it named no conversation and came as a wake was in flight; it was not stopped".into()),
        };
        let identity = serde_json::to_string(&(None::<&Path>, &short)).map_err(failure)?;
        let body = crate::records::encode(&copy).map_err(failure)?;
        node.database
            .write_now_at(Priority::Interactive, body.len(), move |db| {
                db.execute(
                    "INSERT INTO claude_copies(identity,short,body) VALUES(?1,?2,?3) ON CONFLICT(identity) DO NOTHING",
                    (&identity, &short, body),
                )?;
                Ok(())
            })
            .map_err(failure)?;
        node.log.line(format_args!(
            "kept out the Claude background session {session}: it names no conversation and came as a wake was in flight, as the copy Claude Code starts in place of waking a session does"
        ));
        Ok(true)
    }
}

pub(crate) fn of(session: &str, printed: &str) -> Option<String> {
    background::started(printed).filter(|short| !session.starts_with(short.as_str()))
}

#[derive(Clone, Serialize, Deserialize)]
struct Copy {
    /// When it was excluded, in seconds of the epoch.
    #[serde(default)]
    at: i64,
    home: Option<PathBuf>,
    short: String,
    removed: bool,
    seen: bool,
    problem: Option<String>,
}

fn failure(error: impl std::fmt::Display) -> Failure {
    Failure::new(format!("keeping Claude copy exclusion: {error}"))
}

/// The permanent exclusion and cleanup job commit together before any stop or rm.
pub(crate) async fn discard(node: &Arc<Node>, home: Option<&Path>, short: &str) -> Result<(), Failure> {
    let copy = Copy {
        at: jiff::Timestamp::now().as_second(),
        home: home.map(Path::to_path_buf),
        short: short.to_string(),
        removed: false,
        seen: false,
        problem: None,
    };
    let key = serde_json::to_string(&(home, short)).map_err(failure)?;
    let (identity, kept) = (key.clone(), copy.clone());
    let body = crate::records::encode(&kept).map_err(failure)?;
    node.database
        .write_now_at(Priority::Interactive, body.len(), move |db| {
            db.execute(
                "INSERT INTO claude_copies(identity,short,body) VALUES(?1,?2,?3) ON CONFLICT(identity) DO NOTHING",
                (&identity, &kept.short, body),
            )?;
            fastctx_jobs::submit(
                db,
                &fastctx_jobs::Job {
                    kind: KIND,
                    key: identity.clone(),
                    unit: format!("claude-copy/{identity}"),
                    payload: Vec::new(),
                },
            )
        })
        .map_err(failure)?;
    node.durable_jobs.stir(KIND);
    node.log
        .line(format_args!("Claude Code started the copy {short}; its permanent exclusion and cleanup are recorded"));
    clean(node, &key).await
}

async fn clean(node: &Arc<Node>, key: &str) -> Result<(), Failure> {
    let gate = node.claude.copies.cleaning.get_or_insert_with(&key.to_string(), tokio::sync::Mutex::default);
    let _cleaning = gate.lock().await;
    let Some(mut copy) = load(node, key)? else { return Ok(()) };
    if copy.removed {
        return Ok(());
    }
    let result = async {
        let listed =
            super::management::run(node, copy.home.as_deref(), vec!["agents".into(), "--json".into(), "--all".into()])
                .await?;
        let rows: Vec<serde_json::Value> = serde_json::from_str(&listed).map_err(failure)?;
        let record = rows.iter().find(|row| row["kind"] == "background" && row["id"] == copy.short);
        if record.is_none() {
            return if copy.seen {
                Ok(())
            } else if jiff::Timestamp::now().as_second().saturating_sub(copy.at) >= NEVER_LISTED {
                node.log.line(format_args!(
                    "Claude Code never listed the excluded copy {}; it stays excluded and there is nothing to stop",
                    copy.short
                ));
                Ok(())
            } else {
                Err(Failure::new(format!(
                    "excluded Claude copy {} is not yet listed; its cleanup is still pending",
                    copy.short
                )))
            };
        }
        if !copy.seen {
            copy.seen = true;
            save(node, key, &copy)?;
        }
        super::management::run(node, copy.home.as_deref(), vec!["stop".into(), copy.short.clone().into()]).await?;
        super::management::run(node, copy.home.as_deref(), vec!["rm".into(), copy.short.clone().into()]).await?;
        let listed =
            super::management::run(node, copy.home.as_deref(), vec!["agents".into(), "--json".into(), "--all".into()])
                .await?;
        let rows: Vec<serde_json::Value> = serde_json::from_str(&listed).map_err(failure)?;
        if rows.iter().any(|row| row["kind"] == "background" && row["id"] == copy.short) {
            return Err(Failure::new(format!("Claude still lists excluded copy {} after stop and rm", copy.short)));
        }
        Ok(())
    }
    .await;
    copy.removed = result.is_ok();
    let problem = result.as_ref().err().map(|error| error.message.clone());
    if problem != copy.problem
        && let Some(problem) = &problem
    {
        node.log.line(format_args!("Claude copy {} remains; cleanup will retry: {problem}", copy.short));
    }
    copy.problem = problem;
    save(node, key, &copy)?;
    result
}

fn load(node: &Node, key: &str) -> Result<Option<Copy>, Failure> {
    node.database
        .read_now(|db| {
            let body: Option<Vec<u8>> =
                db.query_row("SELECT body FROM claude_copies WHERE identity=?1", [key], |row| row.get(0)).optional()?;
            body.as_deref().map(crate::records::decode::<Copy>).transpose()
        })
        .map_err(failure)
}

fn save(node: &Node, key: &str, copy: &Copy) -> Result<(), Failure> {
    let (key, body) = (key.to_string(), crate::records::encode(copy).map_err(failure)?);
    node.database
        .write_now_at(Priority::Interactive, body.len(), move |db| {
            db.execute("UPDATE claude_copies SET body=?2 WHERE identity=?1", (key, body))?;
            Ok(())
        })
        .map_err(failure)
}

pub(super) fn register(node: &Arc<Node>) {
    let weak = Arc::downgrade(node);
    let run = move |taken: Taken| -> Pin<Box<dyn Future<Output = Outcome> + Send>> {
        let node = weak.upgrade();
        Box::pin(async move {
            let Some(node) = node else { return Outcome::Failed("the node is stopping".into()) };
            let copy = load(&node, &taken.key);
            match copy {
                Ok(None) => Outcome::Done,
                Ok(Some(copy)) if copy.removed => Outcome::Done,
                Ok(Some(copy)) => match clean(&node, &taken.key).await {
                    Ok(()) => Outcome::Done,
                    // Often while it is being started, seldom once it stays.
                    Err(_) => Outcome::Again {
                        after: Duration::from_secs(match jiff::Timestamp::now().as_second().saturating_sub(copy.at) {
                            ..120 => 10,
                            120..3600 => 60,
                            _ => 600,
                        }),
                    },
                },
                Err(error) => {
                    node.log.line(format_args!("reading Claude copy cleanup: {error}"));
                    Outcome::Again { after: Duration::from_secs(10) }
                }
            }
        })
    };
    node.durable_jobs
        .register(Kind { name: KIND, at_once: 4, within: Duration::from_secs(240), tries: 5 }, Arc::new(run));
}
