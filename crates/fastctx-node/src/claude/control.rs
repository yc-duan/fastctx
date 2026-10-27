//! Conversation controls and pending human requests for the Claude bridge.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use super::control_outcome;
use fastctx_hosts::claude::control::{self as files, Answer, Host, Waiting};
use fastctx_types::ResidentId;
use fastctx_types::ipc::Failure;
use fastctx_types::session::control::{Control, ControlKind, Controlled, Offers};
use fastctx_types::session::{Happened, SessionChange, SessionEvent};
use jiff::Timestamp;
use serde_json::{Value, json};

use crate::Node;

#[derive(Default)]
pub struct Bridge {
    calls: Mutex<HashSet<String>>,
    requests: Mutex<HashMap<String, Request>>,
    observed: Mutex<HashSet<(String, String, fastctx_types::ProcessIdentity)>>,
    /// The conversations whose mod is being asked what it offers now.
    offering: Mutex<HashSet<ResidentId>>,
}

/// The one asking out of what a conversation's mod offers, until it ends.
struct Offering<'a> {
    bridge: &'a Bridge,
    resident: ResidentId,
}

impl Bridge {
    /// Begins asking what `resident`'s mod offers, unless an asking is out already.
    fn offering(&self, resident: &ResidentId) -> Option<Offering<'_>> {
        let begun = self.offering.lock().expect("never poisoned").insert(resident.clone());
        begun.then(|| Offering { bridge: self, resident: resident.clone() })
    }
}

impl Drop for Offering<'_> {
    fn drop(&mut self) {
        self.bridge.offering.lock().expect("never poisoned").remove(&self.resident);
    }
}

#[derive(Clone)]
struct Request {
    resident: ResidentId,
    directory: PathBuf,
    waiting: Waiting,
    completed: Arc<tokio::sync::watch::Sender<Option<Result<Answer, Failure>>>>,
}

pub async fn control(node: &Arc<Node>, resident: &ResidentId, control: Control) -> Result<Controlled, Failure> {
    let (about, _) = node.residents.about(resident).ok_or_else(|| Failure::new("unknown conversation"))?;
    if crate::assists::assisting(node, &about.acting) {
        crate::assists::host::require(node, resident)?;
    }
    if crate::assists::assisting(node, &about.acting)
        && let Control::Command { name, path, .. } = &control
        && (path.is_some() || !fastctx_hosts::claude::assisted::command_allowed(name))
    {
        return Err(Failure::not_run(fastctx_hosts::claude::assisted::COMMAND_REFUSED));
    }
    super::resume::ready(node, resident).await?;
    match control {
        Control::Fork { through } => super::fork::fork(node, resident, through).await,
        Control::Answer { request, answer } => answer_request(node, resident, request, answer).await,
        Control::Rewind { .. } if !REWIND_SUPPORTED => Err(Failure::not_run(
            "Claude Code has no supported in-place rewind; fork through the selected turn instead",
        )),
        Control::Rewind { from } => {
            super::history::wait_stored(
                node,
                &node
                    .residents
                    .about(resident)
                    .and_then(|(about, _)| about.session)
                    .ok_or_else(|| Failure::new("Claude has not identified this conversation"))?,
            )
            .await
            .map_err(Failure::new)?;
            let result = invoke_action(node, resident, json!({"do":"rewind", "from":from})).await?;
            if result["kind"] == "rewound" {
                super::rewind::finish(node, resident, &result, Some(&from)).await?
            }
            serde_json::from_value(result).map_err(|error| Failure::new(format!("Claude control reply: {error}")))
        }
        control => {
            let result =
                invoke_action(node, resident, serde_json::to_value(control).expect("control serializes")).await?;
            serde_json::from_value(result).map_err(|error| Failure::new(format!("Claude control reply: {error}")))
        }
    }
}

/// Reads the running host's current local summary, without waking a closed session.
pub(crate) async fn usage_now(node: &Arc<Node>, resident: &ResidentId) -> Result<Value, Failure> {
    if !running(node, resident)? {
        return Err(Failure::new("the Claude session is not running"));
    }
    tokio::time::timeout(std::time::Duration::from_secs(60), invoke(node, resident, json!({"do":"usage"})))
        .await
        .map_err(|_| Failure::new("Claude did not report current usage within 60 seconds"))?
}

/// The goal's compaction respects the host's automatic compaction setting.
pub(crate) async fn goal_compact(node: &Arc<Node>, resident: &ResidentId) -> Result<Controlled, Failure> {
    super::resume::ready(node, resident).await?;
    let result = invoke_action(node, resident, json!({"do":"goal_compact"})).await?;
    serde_json::from_value(result).map_err(|error| Failure::new(format!("Claude compaction reply: {error}")))
}

/// How long a running session's mod is given to say what it offers.
const OFFERS_WITHIN: std::time::Duration = std::time::Duration::from_secs(5);

/// Whether the session's mod runs now with the control contract this node
/// speaks.
pub(super) fn running(node: &Node, resident: &ResidentId) -> Result<bool, Failure> {
    let (about, _) = node.residents.about(resident).ok_or_else(|| Failure::new("unknown resident"))?;
    let Some(session) = &about.session else { return Ok(false) };
    let directory = files::directory(node.world.path(), session);
    match files::host(&directory).map_err(failure)? {
        Some(host) => Ok(files::active(&directory, &host).map_err(failure)?
            && host.command_contract == Some(files::COMMAND_CONTRACT)),
        None => Ok(false),
    }
}

pub async fn offers(node: &Arc<Node>, resident: &ResidentId) -> Result<Offers, Failure> {
    let (about, _) = node.residents.about(resident).ok_or_else(|| Failure::new("unknown resident"))?;
    let mut running = false;
    if let Some(session) = &about.session {
        let directory = files::directory(node.world.path(), session);
        if let Some(host) = files::host(&directory).map_err(failure)? {
            running = files::active(&directory, &host).map_err(failure)?
                && host.command_contract == Some(files::COMMAND_CONTRACT);
        }
    }
    // The models are the World's own table (`fastctx_hosts::claude::models`):
    // the picker is there whatever the session's mod answers. A running
    // mod adds its commands and checkpoints, and names the aliases its
    // login offers; when it does not answer, or not soon, as in the middle
    // of a turn, the conversation is offered as a sleeping one is. A reply
    // that comes later is cleared away by `look`. The mod is asked once at a
    // time: pages and machines asking together while one asking is out are
    // answered at once the sleeping way, from what the World keeps, and take
    // nothing from the session: an interface asking dozens of times in a few
    // seconds would otherwise hold every place a busy machine gives it.
    let asked = if running {
        match node.claude.bridge.offering(resident) {
            Some(_offering) => {
                match tokio::time::timeout(OFFERS_WITHIN, invoke(node, resident, json!({"do":"offers"}))).await {
                    Ok(answer) => answer.and_then(|value| {
                        serde_json::from_value::<Offers>(value)
                            .map_err(|error| Failure::new(format!("Claude offers: {error}")))
                    }),
                    Err(_) => Err(Failure::new("Claude did not say what it offers in time")),
                }
            }
            None => Err(Failure::new("what it offers is being asked already")),
        }
    } else {
        Err(Failure::new("not running"))
    };
    let mut offers = if let Ok(mut offers) = asked {
        super::catalog::keep(node, resident, &offers)?;
        // The mod lists the aliases this login's Claude Code offers; the
        // World offers its own table's rows of those families.
        let aliases: Vec<String> = offers.models.drain(..).map(|model| model.id).collect();
        offers.models = fastctx_hosts::claude::models::model_offers(Some(&aliases));
        if !offers.controls.contains(&ControlKind::Model) {
            offers.controls.push(ControlKind::Model);
        }
        offers
    } else {
        let selected = about
            .session
            .as_ref()
            .map(|session| files::read::<Value>(&files::directory(node.world.path(), session).join("selection.json")))
            .transpose()
            .map_err(failure)?
            .flatten();
        let initial = crate::session_options::read(node, resident)?;
        let checkpoints = about
            .session
            .as_ref()
            .map(|session| files::read_message(&files::directory(node.world.path(), session).join("checkpoints.json")))
            .transpose();
        let checkpoints = match checkpoints {
            Ok(value) => value.filter(|value| value["full"] == true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(failure(error)),
        };
        // A sleeping session's model is chosen as a running one's is: kept
        // for its next turn, among the models its login was last seen to
        // offer. One chosen before Claude Code names the session, in the few
        // seconds it starts, waits for the name (`named`).
        let aliases = about
            .session
            .as_ref()
            .map(|session| {
                fastctx_hosts::claude::exchange::observed_aliases(
                    &fastctx_hosts::claude::exchange::sessions(node.world.path()).join(session),
                )
            })
            .transpose()
            .map_err(failure)?
            .flatten();
        let catalog = super::catalog::get(node, resident, about.session.as_deref())?;
        Offers {
            controls: catalog.controls,
            commands: catalog.commands,
            checkpoints: checkpoints.as_ref().map(|value| {
                value["turns"]
                    .as_object()
                    .into_iter()
                    .flatten()
                    .map(|(turn, point)| fastctx_types::session::control::TurnCheckpoint {
                        turn: turn.clone(),
                        rewind: false,
                        fork: point["after"].as_u64().is_some_and(|count| count > 0) && point["after_uuid"].is_string(),
                    })
                    .collect()
            }),
            history_limits: Some(fastctx_types::session::control::HistoryLimits {
                files_restored: false,
                compacted_turns_reachable: false,
                empty_prefix_allowed: false,
                complete_prefix: checkpoints.is_some(),
            }),
            model: selected
                .as_ref()
                .and_then(|value| value["model"].as_str())
                .map(str::to_owned)
                .or(initial.model)
                .or(about.model),
            effort: selected
                .as_ref()
                .and_then(|value| value["effort"].as_str())
                .map(str::to_owned)
                .or(initial.reasoning_effort),
            models: fastctx_hosts::claude::models::model_offers(aliases.as_deref()),
            ..Default::default()
        }
    };
    if !REWIND_SUPPORTED {
        offers.controls.retain(|control| *control != ControlKind::Rewind);
        if let Some(points) = &mut offers.checkpoints {
            for point in points {
                point.rewind = false;
            }
        }
    }
    if about.session.is_some() && super::command(node, about.home.as_deref()).is_ok() {
        offers.controls.push(ControlKind::Fork);
    }
    if node.claude.bridge.requests.lock().expect("never poisoned").values().any(|request| &request.resident == resident)
    {
        offers.controls.push(ControlKind::Answer);
    }
    if crate::assists::assisting(node, &about.acting) {
        if crate::assists::host::kept(node, resident)? != Some(true) {
            offers.controls.clear();
            offers.commands.clear();
            return Ok(offers);
        }
        offers.commands.retain(|command| {
            command.path.is_none() && fastctx_hosts::claude::assisted::command_allowed(&command.name)
        });
    }
    Ok(offers)
}

/// Claude Code has no entrance that rewinds in place and keeps the context:
/// `session.compact` from a plugin skips that plugin's own compaction
/// handler and asks the model to compact, and running the compact command
/// reaches the handler, but a native cold fork of the result repeats the
/// assistant's retained messages. Rewind is offered once one keeps it whole.
const REWIND_SUPPORTED: bool = false;

pub(super) fn live_host(node: &Node, resident: &ResidentId) -> Result<(PathBuf, Host), Failure> {
    let (about, _) = node.residents.about(resident).ok_or_else(|| Failure::new("unknown resident"))?;
    let session = about.session.ok_or_else(|| Failure::new("Claude has not identified this conversation yet"))?;
    let directory = files::directory(node.world.path(), &session);
    let host = files::host(&directory).map_err(failure)?.ok_or_else(|| {
        Failure::new("this Claude session has no active World control bridge; reopen it with the current plugin")
    })?;
    if !files::active(&directory, &host).map_err(failure)? {
        return Err(Failure::not_run("this Claude session retired before the control was submitted"));
    }
    if host.command_contract != Some(files::COMMAND_CONTRACT) {
        return Err(Failure::new(
            "this Claude session's World mod still has the old terminal command contract; reload it before using conversation controls",
        ));
    }
    Ok((directory, host))
}

pub(super) async fn selection(
    node: &Arc<Node>,
    resident: &ResidentId,
    running: bool,
) -> Result<fastctx_types::agents::SessionOptions, Failure> {
    let initial = crate::session_options::read(node, resident)?;
    let selected = if running {
        Some(invoke(node, resident, json!({"do":"selection"})).await?)
    } else {
        let (about, _) = node.residents.about(resident).ok_or_else(|| Failure::new("unknown resident"))?;
        about
            .session
            .as_ref()
            .map(|session| files::read::<Value>(&files::directory(node.world.path(), session).join("selection.json")))
            .transpose()
            .map_err(failure)?
            .flatten()
    };
    Ok(fastctx_types::agents::SessionOptions {
        mcp_off: initial.mcp_off,
        model: selected.as_ref().and_then(|value| value["model"].as_str()).map(str::to_owned).or(initial.model),
        reasoning_effort: selected
            .as_ref()
            .and_then(|value| value["effort"].as_str())
            .map(str::to_owned)
            .or(initial.reasoning_effort),
    })
}

async fn invoke_action(node: &Arc<Node>, resident: &ResidentId, control: Value) -> Result<Value, Failure> {
    invoke(node, resident, control).await
}

pub(super) async fn invoke(node: &Arc<Node>, resident: &ResidentId, control: Value) -> Result<Value, Failure> {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(120);
    let (directory, host) = loop {
        match live_host(node, resident) {
            Err(error) if error.not_run && control["do"] != "offers" && tokio::time::Instant::now() < deadline => {
                super::resume::ready(node, resident).await?;
            }
            result => break result?,
        }
    };
    let key = crate::speech::new_id("control").to_string();
    let path = directory.join("inbox").join(format!("{key}.json"));
    let handoff = host.handoff_contract == Some(1);
    let mut evidence = control_outcome::Evidence::capture(&directory, &control)
        .map_err(|error| Failure::not_run(format!("preparing Claude control evidence: {error}")))?;
    node.claude.bridge.calls.lock().expect("never poisoned").insert(key.clone());
    struct Pending<'a> {
        node: &'a Node,
        key: &'a str,
    }
    impl Drop for Pending<'_> {
        fn drop(&mut self) {
            let mut calls = self.node.claude.bridge.calls.lock().expect("never poisoned");
            calls.remove(self.key);
            if calls.is_empty() {
                *calls = HashSet::new();
            }
        }
    }
    let _pending = Pending { node, key: &key };
    let result = fastctx_platform::current_identity().and_then(|controller| {
        files::write_message(&path, &json!({"instance":host.instance,"controller":controller,"control":control,"handoff_contract":if handoff {1} else {0}}))
    });
    if let Err(error) = result {
        node.claude.bridge.calls.lock().expect("never poisoned").remove(&key);
        return Err(failure(error));
    }
    let within = if control_outcome::compacts(&control) {
        crate::control_queue::COMPACTING
    } else {
        std::time::Duration::from_secs(120)
    };
    let began = std::time::Instant::now();
    let _outside = fastctx_platform::doing::waiting_outside("Claude control receipt", within);
    loop {
        if handoff
            && files::read::<files::Claim>(&directory.join("claims").join(format!("{key}.json")))
                .map_err(|error| control_outcome::unknown(&format!("reading Claude's control claim: {error}")))?
                .filter(|claim| !claim.released)
                .is_none()
            && !running(node, resident).map_err(|error| control_outcome::unknown(&error.message))?
        {
            super::resume::ready(node, resident).await.map_err(|error| {
                control_outcome::unknown(&format!("reconnecting the submitted Claude control: {}", error.message))
            })?;
        }
        let result = {
            let gate = super::reading(node, &host.session);
            let _reading = gate.lock().await;
            let reply = directory.join("replies").join(format!("{key}.json"));
            let claimed = files::read::<files::Claim>(&directory.join("claims").join(format!("{key}.json")))
                .map_err(|error| control_outcome::unknown(&format!("reading Claude's control claim: {error}")))?
                .filter(|claim| !claim.released);
            if let Some(claim) = &claimed {
                evidence.executing(claim).map_err(|error| {
                    control_outcome::unknown(&format!("reading Claude's claimed evidence: {error}"))
                })?;
            }
            let executing = claimed.as_ref().map(|claim| &claim.host).unwrap_or(&host);
            let result = if reply.with_extension("done").exists() {
                Some(
                    files::read_message(&reply)
                        .map_err(|error| control_outcome::unknown(&format!("reading Claude's receipt: {error}")))
                        .and_then(control_outcome::reply),
                )
            } else if handoff && claimed.is_none() {
                None
            } else if directory.join("closed").join(format!("{}.json", executing.instance)).exists()
                || !fastctx_platform::is_alive(&executing.process)
                    .map_err(|error| control_outcome::unknown(&error.to_string()))?
            {
                match evidence.recovered(&directory, &key, executing) {
                    Ok(Some(result)) => Some(Ok(result)),
                    Err(error) => {
                        Some(Err(control_outcome::unknown(&format!("checking Claude's control evidence: {error}"))))
                    }
                    Ok(None)
                        if evidence.may_finish().map_err(|error| control_outcome::unknown(&error.to_string()))? =>
                    {
                        None
                    }
                    Ok(None) => Some(Err(control_outcome::unknown("the Claude activation ended without a receipt"))),
                }
            } else {
                None
            };
            if result.is_some()
                && let Err(error) = collect_invocation(&directory, &key)
            {
                node.log
                    .line(format_args!("collecting Claude control {key} after its outcome was established: {error}"));
            }
            result
        };
        if let Some(result) = result {
            return result;
        }
        if began.elapsed() >= within {
            return Err(control_outcome::unknown("Claude did not confirm the control before its deadline"));
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
}

async fn answer_request(
    node: &Node,
    resident: &ResidentId,
    address: Value,
    answer: Value,
) -> Result<Controlled, Failure> {
    let request = node
        .claude
        .bridge
        .requests
        .lock()
        .expect("never poisoned")
        .get(&address.to_string())
        .filter(|request| &request.resident == resident)
        .cloned()
        .ok_or_else(|| Failure::not_run("this Claude request is no longer waiting, or belongs to another resident"))?;
    let mut completed = request.completed.subscribe();
    let question = &request.waiting.question;
    let directory = files::directory(node.world.path(), &question.host.session);
    if !files::active(&directory, &question.host).map_err(failure)?
        || !fastctx_platform::is_alive(&request.waiting.process).map_err(failure)?
        || request.directory.join("resolved.json").exists()
        || request.directory.join("cancel.json").exists()
    {
        return Err(Failure::not_run("Claude already finished or cancelled this request"));
    }
    files::validate_answer(question, &answer).map_err(Failure::not_run)?;
    files::answer(&request.directory, &Answer::Answer { answer: answer.clone() }).map_err(|error| {
        if error.kind() == std::io::ErrorKind::AlreadyExists {
            Failure::new("this Claude request already has an answer")
        } else {
            failure(error)
        }
    })?;
    loop {
        if let Some(resolution) = completed.borrow_and_update().clone() {
            return match resolution? {
                Answer::Answer { answer: received } if received == answer => Ok(Controlled::Done { said: None }),
                Answer::Pass { reason } => {
                    Err(Failure::new(format!("Claude stopped waiting before taking this answer: {reason}")))
                }
                _ => Err(Failure::new("Claude took a different answer")),
            };
        }
        completed
            .changed()
            .await
            .map_err(|_| control_outcome::unknown("the Claude request bridge stopped before confirming this answer"))?;
    }
}

/// Called by the existing session-directory watcher, after pairing. No
/// control response is interpreted as a transcript or as a new model turn.
pub async fn look(node: &Arc<Node>, resident: &ResidentId, session_directory: &Path) -> std::io::Result<()> {
    let directory = session_directory.join("control");
    let Some(host) = files::host(&directory)? else { return Ok(()) };
    if !session_present(node, session_directory)? {
        ended(node, &host.session);
        return Ok(());
    }
    observe_activation(node, &host, session_directory);

    for path in crate::retention::entries(&directory.join("replies"))? {
        if path.extension().is_some_and(|extension| extension == "done")
            && let Some(key) = path.file_stem().and_then(|key| key.to_str())
            && !node.claude.bridge.calls.lock().expect("never poisoned").contains(key)
        {
            super::goal_compaction::capture(node, resident, &directory, key)?;
            collect_invocation(&directory, key)?;
        }
    }
    let entries = match std::fs::read_dir(directory.join("requests")) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let path = entry?.path();
        if !path.is_dir() {
            continue;
        }
        let Some(waiting) = files::read::<Waiting>(&path.join("request.json"))? else { continue };
        if waiting.question.directory(&directory)? != path {
            return Err(std::io::Error::other("Claude request directory does not match its address"));
        }
        let key = waiting.question.address().to_string();
        if let Some(resolution) = files::read::<Answer>(&path.join("resolved.json"))? {
            resolved(node, &key, resolution)?;
            if !fastctx_platform::is_alive(&waiting.process)? {
                fastctx_platform::owned_directory::delete(&path)?;
            }
            continue;
        }
        if waiting.question.host != host
            || !files::active(&directory, &host)?
            || !fastctx_platform::is_alive(&waiting.process)?
        {
            unconfirmed(node, &key, &path, "the Claude request's process or activation ended")?;
            continue;
        }
        let (completed, _) = tokio::sync::watch::channel(None);
        let request = Request { resident: resident.clone(), directory: path, waiting, completed: Arc::new(completed) };
        let fresh = {
            let mut requests = node.claude.bridge.requests.lock().expect("never poisoned");
            if requests.contains_key(&key) {
                false
            } else {
                requests.insert(key.clone(), request.clone());
                true
            }
        };
        if fresh {
            changed(
                node,
                &request,
                SessionChange::Request {
                    id: request.waiting.question.address(),
                    request: request.waiting.question.request.clone(),
                    params: request.waiting.question.params.clone(),
                },
            );
            waiting_state(node, resident)?;
            supervise(node, key, request);
        }
    }
    waiting_state(node, resident)
}

pub(super) fn collect_invocation(directory: &Path, key: &str) -> std::io::Result<()> {
    for side in ["inbox", "replies", "retained", "started"] {
        for extension in ["json", "done", "parts"] {
            match fastctx_platform::owned_directory::delete(&directory.join(side).join(format!("{key}.{extension}"))) {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error),
                _ => {}
            }
        }
    }
    Ok(())
}

fn observe_activation(node: &Arc<Node>, host: &Host, directory: &Path) {
    let key = (host.session.clone(), host.instance.clone(), host.process.clone());
    if !node.claude.bridge.observed.lock().expect("never poisoned").insert(key.clone()) {
        return;
    }
    let process = host.process.clone();
    let host = host.clone();
    let node = node.clone();
    let directory = directory.to_path_buf();
    tokio::spawn(async move {
        let exited = super::process::wait(&node, &process);
        tokio::pin!(exited);
        let result = loop {
            let changed = node.claude.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let removed = node
                .residents
                .resident_of_thread(&host.session)
                .is_some_and(|resident| node.residents.removed(&resident).is_some());
            match session_present(&node, &directory) {
                Ok(false) => break Ok(()),
                Ok(true) => {}
                Err(error) => break Err(error),
            }
            match files::active(&files::directory(node.world.path(), &host.session), &host) {
                Ok(false) => break Ok(()),
                Ok(true) if removed => break Ok(()),
                Ok(true) => {}
                Err(error) => break Err(error),
            }
            tokio::select! { () = &mut exited => break Ok(()), () = changed => {} }
        };
        match result {
            Ok(()) => super::look(&node, &directory).await,
            result => node.log.line(format_args!("observing Claude's control activation: {result:?}")),
        }
        let mut observed = node.claude.bridge.observed.lock().expect("never poisoned");
        observed.remove(&key);
        if observed.is_empty() {
            *observed = HashSet::new();
        }
    });
}

fn session_present(node: &Node, directory: &Path) -> std::io::Result<bool> {
    match fastctx_hosts::claude::exchange::registration(directory)? {
        Some(registration) => super::pairing::present(node, &registration),
        None => Ok(false),
    }
}

fn supervise(node: &Arc<Node>, key: String, request: Request) {
    let node = node.clone();
    tokio::spawn(async move {
        let process = request.waiting.process.clone();
        let exited = super::process::wait(&node, &process);
        let mut completed = request.completed.subscribe();
        tokio::pin!(exited);
        tokio::select! {
            _ = async { let _ = completed.wait_for(Option::is_some).await; } => return,
            () = &mut exited => {}
            () = node.watchers.unwatched(&request.resident) => {
                let pass = Answer::Pass { reason: "no one is watching this conversation in the World".into() };
                if let Err(error) = files::answer(&request.directory, &pass)
                    && error.kind() != std::io::ErrorKind::AlreadyExists {
                    node.log.line(format_args!("returning Claude's request to its host: {error}"));
                }
                tokio::select! {
                    () = &mut exited => {},
                    _ = async { let _ = completed.wait_for(Option::is_some).await; } => return,
                }
            }
        }
        let result = unconfirmed(&node, &key, &request.directory, "Claude stopped waiting for this request");
        if let Err(error) = result {
            node.log.line(format_args!("settling Claude's request: {error}"));
        }
    });
}

fn unconfirmed(node: &Node, key: &str, directory: &Path, reason: &str) -> std::io::Result<()> {
    let answer = match files::read::<Answer>(&directory.join("resolved.json"))? {
        Some(answer) => Ok(answer),
        None if matches!(files::read::<Answer>(&directory.join("answer.json"))?, Some(Answer::Answer { .. })) => {
            Err(control_outcome::unknown(reason))
        }
        None => Ok(Answer::Pass { reason: reason.into() }),
    };
    settled(node, key, answer)
}

fn resolved(node: &Node, key: &str, answer: Answer) -> std::io::Result<()> {
    settled(node, key, Ok(answer))
}

fn settled(node: &Node, key: &str, answer: Result<Answer, Failure>) -> std::io::Result<()> {
    let request = {
        let mut requests = node.claude.bridge.requests.lock().expect("never poisoned");
        let request = requests.remove(key);
        if requests.is_empty() {
            *requests = HashMap::new();
        }
        request
    };
    if let Some(request) = request {
        request.completed.send_replace(Some(answer.clone()));
        changed(
            node,
            &request,
            SessionChange::Resolved {
                id: request.waiting.question.address(),
                resolution: match answer {
                    Ok(answer) => serde_json::to_value(answer).expect("answer serializes"),
                    Err(error) => json!({"kind":"unknown", "reason":error.message}),
                },
            },
        );
        waiting_state(node, &request.resident)?;
    }
    Ok(())
}

/// Whether the resident waits on the user, and for what: the latest of its
/// requests still open.
fn waiting_state(node: &Node, resident: &ResidentId) -> std::io::Result<()> {
    let requests = node.claude.bridge.requests.lock().expect("never poisoned");
    let latest =
        requests.values().filter(|request| &request.resident == resident).max_by_key(|request| request.waiting.at);
    let waiting = latest.is_some();
    let asked = latest.and_then(|request| {
        let question = &request.waiting.question;
        crate::asked::of_request(&question.address(), &question.request, &question.params, request.waiting.at)
    });
    let now = node.residents.now(resident);
    let asked = match asked {
        Some(asked) => Some(asked),
        // A question asked without waiting is not settled by a request.
        None => now.as_ref().and_then(|now| now.asked.clone()).filter(|asked| !asked.waits),
    };
    if now.is_some_and(|now| now.waiting_for_user == waiting && now.asked == asked) {
        return Ok(());
    }
    node.residents.change_now(&node.world, resident, |now| {
        now.waiting_for_user = waiting;
        now.asked = asked;
    })
}

fn changed(node: &Node, request: &Request, change: SessionChange) {
    node.watchers.publish(
        &request.resident,
        &SessionEvent {
            at: Timestamp::now(),
            happened: Happened::Changed { turn: request.waiting.question.turn.clone(), change },
        },
    );
}

pub fn ended(node: &Node, session: &str) {
    node.claude.changed.notify_waiters();

    let requests: Vec<_> = node
        .claude
        .bridge
        .requests
        .lock()
        .expect("never poisoned")
        .iter()
        .filter(|(_, request)| request.waiting.question.host.session == session)
        .map(|(key, request)| (key.clone(), request.clone()))
        .collect();
    for (key, request) in requests {
        let answer = Answer::Pass { reason: "the Claude conversation ended".into() };
        if let Err(error) = files::answer(&request.directory, &answer)
            && error.kind() != std::io::ErrorKind::AlreadyExists
        {
            node.log.line(format_args!("releasing Claude's answer helper: {error}"));
        }
        if let Err(error) = unconfirmed(node, &key, &request.directory, "the Claude conversation ended") {
            node.log.line(format_args!("settling Claude's interrupted request: {error}"));
        }
    }
}

fn failure(error: std::io::Error) -> Failure {
    Failure::new(error.to_string())
}
