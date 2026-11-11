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
}

pub(crate) fn of(session: &str, printed: &str) -> Option<String> {
    background::started(printed).filter(|short| !session.starts_with(short.as_str()))
}

#[derive(Clone, Serialize, Deserialize)]
struct Copy {
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
                Ok(Some(_)) => match clean(&node, &taken.key).await {
                    Ok(()) => Outcome::Done,
                    Err(_) => Outcome::Again { after: Duration::from_secs(10) },
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
