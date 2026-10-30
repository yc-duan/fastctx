//! Where a resident's scripts start (`world-run.md`, 这里): the machine it
//! lives on. A script that is a single `on MACHINE` and nothing else has
//! nothing to do there, so it is handed whole, with its wait, to that
//! machine, which runs it as a job of its own, keeps its output there and
//! answers as it would answer there; only the answer crosses. Words that
//! come for the resident where it lives end the wait there as they would
//! here, and a job the answer showed whole goes there once its host has it.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use fastctx_types::access::{Acting, HubAccount, Purpose, Written};
use fastctx_types::ipc::Failure;
use fastctx_types::remote::{RemoteCall, RemoteReply};
use fastctx_types::run::{RunReply, RunRequest, RunState};
use fastctx_types::{Address, JobId, MachineName, ResidentId, TownName};

use crate::Node;

/// The jobs a `run` from here handed to another machine and that answered
/// whole, with the project they went under there, until its host
/// has the answer: then they go there.
#[derive(Default)]
pub struct Dispatched {
    jobs: Mutex<HashMap<(ResidentId, JobId), Went>>,
}

/// Where a job a `run` handed over went: the machine, and the project it is
/// under there.
type Went = (Written, Option<TownName>);

/// Residents being born here, with whom they work for: their host
/// starts their tool server before their record is written, and the tools
/// it lists then must already be theirs, as Codex does not list them again
/// (`world-colleagues.md`, 同事从哪里来、能去哪里).
#[derive(Default)]
pub struct Births(Mutex<HashMap<ResidentId, Born>>);

struct Born {
    acting: Acting,
    user_started: bool,
}

/// A resident whose initial setup is in progress; dropping it ends the birth,
/// whether setup succeeded or failed.
pub(crate) struct Birth {
    node: Arc<Node>,
    id: ResidentId,
}

impl Drop for Birth {
    fn drop(&mut self) {
        self.node.births.0.lock().expect("never poisoned").remove(&self.id);
        crate::goals::engine::input_changed(&self.node, &self.id);
    }
}

/// `id` is being born here, working for `acting`.
pub(crate) fn born(
    node: &Arc<Node>,
    id: &ResidentId,
    acting: Acting,
    origin: &fastctx_types::residents::Origin,
) -> Birth {
    let user_started = matches!(origin, fastctx_types::residents::Origin::User);
    node.births.0.lock().expect("never poisoned").insert(id.clone(), Born { acting, user_started });
    Birth { node: node.clone(), id: id.clone() }
}

/// Initial setup may still be publishing the first input after the resident record exists.
pub(crate) fn being_born(node: &Node, id: &ResidentId) -> bool {
    node.births.0.lock().expect("never poisoned").contains_key(id)
}

/// Whether the World is starting this conversation for the user, before its host identifies it.
pub(crate) fn born_for_user(node: &Node, id: &ResidentId) -> bool {
    node.births.0.lock().expect("never poisoned").get(id).is_some_and(|born| born.user_started)
}

/// Whom `id` works for, as its record says, or as its birth does until the
/// record is written.
pub(crate) fn acting(node: &Node, id: &ResidentId) -> Option<Acting> {
    match node.residents.about(id) {
        Some((about, _)) => Some(about.acting),
        None => node.births.0.lock().expect("never poisoned").get(id).map(|born| born.acting.clone()),
    }
}

/// A job a run of a resident handed to another machine and left running
/// there, kept here until word of its end comes: the line of jobs
/// still running, at the end of each answer, names it (`world-run.md`, 怎样
/// 看它).
#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct Elsewhere {
    pub(crate) machine: Written,
    pub(crate) job: JobId,
    pub(crate) command: String,
    pub(crate) started_at: jiff::Timestamp,
}

fn elsewhere_path(node: &Node, resident: &ResidentId, machine: &MachineName, job: &JobId) -> PathBuf {
    node.world.conversation(resident).elsewhere().join(format!("{}-{job}.json", machine.key()))
}

fn left_running(node: &Node, resident: &ResidentId, kept: &Elsewhere) {
    let path = elsewhere_path(node, resident, &kept.machine.name, &kept.job);
    let bytes = serde_json::to_vec(kept).expect("records serialize");
    let written = fastctx_platform::ensure_private_dir(path.parent().expect("a resident's directory"))
        .and_then(|()| fastctx_platform::publish(&path, &bytes, fastctx_platform::Permissions::Private));
    if let Err(error) = written {
        node.log.line(format_args!("keeping {} as still running: {error}", kept.job));
    }
}

/// Word of the end of `job` on `machine` came for `resident`.
pub(crate) fn ended_elsewhere(node: &Node, resident: &ResidentId, machine: &MachineName, job: &JobId) {
    match std::fs::remove_file(elsewhere_path(node, resident, machine, job)) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            node.log.line(format_args!("forgetting {job} on {machine} as still running: {error}"));
        }
        _ => {}
    }
}

/// What runs of `resident` left running on other machines.
pub(crate) fn running_elsewhere(node: &Node, resident: &ResidentId) -> Vec<Elsewhere> {
    let Ok(entries) = std::fs::read_dir(node.world.conversation(resident).elsewhere()) else {
        return Vec::new();
    };
    entries
        .filter_map(|entry| std::fs::read(entry.ok()?.path()).ok())
        .filter_map(|bytes| serde_json::from_slice(&bytes).ok())
        .collect()
}

/// Every machine `script` names literally after `on` must be one this
/// machine can reach for `acting` now, known, online and allowed, before
/// any of it runs: otherwise a script would leave half its work done
/// (`world-run.md`, 去别的机器). A name in a variable is checked only when its
/// `on` runs, and so is a machine that leaves after this.
pub(crate) fn check_named(node: &Node, acting: &Acting, script: &str) -> Result<(), Failure> {
    let mut checked = std::collections::HashSet::new();
    for (name, line) in fastctx_hands::run::preread::machines_named(script) {
        if !checked.insert(name.clone()) {
            continue;
        }
        let refuse =
            |why: String| Failure::not_run(format!("nothing ran: line {line} names {name} after `on`, and {why}"));
        let written = Written::parse(&name).map_err(|error| refuse(error.to_string()))?;
        if node.is_here(&written) {
            node.authorize_here(acting)?;
            continue;
        }
        let speakers =
            node.speakers(acting, &written, Purpose::Hands).map_err(|failure| refuse(failure.to_string()))?;
        let seen: Vec<(bool, Option<jiff::Timestamp>)> = speakers
            .iter()
            .filter_map(|(link, _)| link.view().member(&written.name).map(|member| (member.online, member.since)))
            .collect();
        if !seen.iter().any(|(online, _)| *online) {
            let last_seen = seen.iter().filter_map(|(_, since)| *since).max();
            let offline = fastctx_net::link::CallFailure::Offline { name: written.name.clone(), last_seen };
            return Err(refuse(offline.to_string()));
        }
    }
    Ok(())
}

/// Where a run goes, and what runs there: every script of a session is one
/// literal `on MACHINE`, its body run on that machine from its home
/// (`world-run.md`, 脚本在哪台机器上跑). `None` is this machine. A script of
/// anything else is refused with why, before anything of it runs.
pub(crate) fn start(node: &Node, request: RunRequest) -> Result<(Option<Written>, RunRequest), String> {
    let (machine, body) = fastctx_hands::run::preread::on_only(&request.script).map_err(|failure| failure.message)?;
    let body =
        body.ok_or("a script is one `on MACHINE`; put the rest inside it, where `on` reaches the other machines")?;
    let machine = Written::parse(&machine).map_err(|error| error.to_string())?;
    let whole = RunRequest { script: body, cwd: None, ..request };
    Ok(((!node.is_here(&machine)).then_some(machine), whole))
}

/// Runs `request` for `resident`, of this machine, on `to`, the one machine
/// a script of one `on` names. It waits there as a run here waits and
/// answers what that machine answered.
pub(crate) async fn run(
    node: &Arc<Node>,
    resident: &ResidentId,
    town: Option<TownName>,
    request: RunRequest,
    to: &Written,
) -> Result<RunReply, Failure> {
    let acting = crate::authorization::resident(node, resident)?;
    let here = Address { machine: node.machine(), resident: resident.clone() };
    let directory = node.world.conversation(resident);
    // Listening before the call goes, so words that come, or an interrupted
    // turn, meanwhile are not missed here.
    let mut words = node.words.watch(&here);
    let mut interrupted = node.interrupts.watch(&here);
    let town = match node.speaking(&acting, to, Purpose::Hands).await {
        Ok((link, _)) => node.town_across(&link, town.as_ref())?,
        Err(_) => town,
    };
    let (script, started_at) = (request.script.clone(), jiff::Timestamp::now());
    let dispatch = crate::speech::new_id("run");
    let call = RemoteCall::Run {
        resident: resident.clone(),
        town: town.clone(),
        request,
        answer_receipts: true,
        dispatch: Some(dispatch.clone()),
    };
    let source = crate::letters::Identity::at(node, to, "");
    let answer = node.call(&acting, to, Purpose::Hands, call);
    tokio::pin!(answer);
    let (mut told, mut stopping) = (false, false);
    let answered = loop {
        tokio::select! {
            answered = &mut answer => break answered,
            // The job there stops with the turn; its answer comes as it ends.
            _ = interrupted.changed(), if !stopping => {
                stopping = true;
                crate::run_stops::keep(node, &here, &acting, to, &dispatch).await
                    .map_err(|error| Failure::new(format!("keeping the turn's stop: {error}")))?;
            }
            // The node holds every sender, so the watch never closes.
            _ = words.changed(), if !told => {
                told = true;
                let (node, acting, to, resident) = (node.clone(), acting.clone(), to.clone(), resident.clone());
                tokio::spawn(async move {
                    // When it does not get there, the run answers when its
                    // wait is over, and the words are read then.
                    let nudge = RemoteCall::WordsCame { resident };
                    if let Err(failure) = node.call(&acting, &to, Purpose::Hands, nudge).await {
                        node.log.line(format_args!("telling {} that words came: {failure}", to.name));
                    }
                });
            }
        }
    };
    if stopping {
        let outcome = match &answered {
            Ok(Ok(RemoteReply::Run(reply))) => crate::run_stops::Answered::Job(reply.job.clone()),
            Ok(Err(failure)) if failure.not_run => crate::run_stops::Answered::NotRun,
            Err(failure) if never_started(failure) => crate::run_stops::Answered::NotRun,
            _ => crate::run_stops::Answered::Unknown,
        };
        // The answer still goes to the model; only an older receiver's stop
        // is left without the job it names.
        if let Err(error) = crate::run_stops::answered(node, &here, &dispatch, outcome) {
            node.log.line(format_args!("keeping the stopped run's answer on {}: {error}", to.name));
        }
    }
    let reply = match answered {
        Ok(Ok(RemoteReply::Run(reply))) => *reply,
        Ok(Ok(_)) => return Err(Failure::new(format!("{} answered something other than a run", to.name))),
        // What went wrong there is said of that machine, where the script
        // was to start.
        Ok(Err(failure)) => {
            return Err(Failure { message: format!("on {}: {}", to.name, failure.message), ..failure });
        }
        Err(failure) => {
            let message = format!("the script starts on {}: {failure}", to.name);
            return Err(if never_started(&failure) { Failure::not_run(message) } else { Failure::unanswered(message) });
        }
    };
    crate::run_receipts::remember(node, &here, &reply).map_err(|error| Failure::new(error.to_string()))?;
    if let Some(consumed) = &reply.consumed {
        crate::job_receipts::apply_consumed(&directory, consumed).map_err(|error| Failure::new(error.to_string()))?;
        if consumed.iter().any(|id| !id.as_str().starts_with("letter-")) {
            let source = source.map_err(|error| Failure::new(error.to_string()))?;
            let current =
                crate::letters::Identity::at(node, to, "").map_err(|error| Failure::new(error.to_string()))?;
            if !source.same_computer(&current) {
                return Err(Failure::new("the job computer's identity changed while its answer was in flight"));
            }
            for id in consumed.iter().filter(|id| !id.as_str().starts_with("letter-")) {
                source
                    .with_id(node, id.as_str())
                    .consume(&directory)
                    .map_err(|error| Failure::new(error.to_string()))?;
            }
        }
    }
    if reply.state != RunState::Running {
        node.dispatched
            .jobs
            .lock()
            .expect("never poisoned")
            .insert((resident.clone(), reply.job.clone()), (to.clone(), town));
    } else {
        let command = crate::jobs::first_line(&script);
        left_running(node, resident, &Elsewhere { machine: to.clone(), job: reply.job.clone(), command, started_at });
    }
    Ok(reply)
}

/// Whether a run's call failed before it could reach the machine it names.
fn never_started(failure: &fastctx_net::link::CallFailure) -> bool {
    matches!(
        failure,
        fastctx_net::link::CallFailure::NotConnected { .. }
            | fastctx_net::link::CallFailure::NotMember { .. }
            | fastctx_net::link::CallFailure::Offline { .. }
            | fastctx_net::link::CallFailure::Refused { .. }
            | fastctx_net::link::CallFailure::NotAllowed { .. }
            | fastctx_net::link::CallFailure::Unreachable { .. }
    )
}

/// The answer that showed `job` whole reached its host: where it ran, if
/// it ran elsewhere, its files go.
pub(crate) fn get_dispatched(node: &Node, resident: &ResidentId, job: &JobId) -> Option<Went> {
    node.dispatched.jobs.lock().expect("never poisoned").get(&(resident.clone(), job.clone())).cloned()
}

pub(crate) fn take_dispatched(node: &Node, resident: &ResidentId, job: &JobId) -> Option<Went> {
    node.dispatched.jobs.lock().expect("never poisoned").remove(&(resident.clone(), job.clone()))
}

pub(crate) fn forget(node: &Arc<Node>, resident: &ResidentId, job: &JobId) -> bool {
    let Some((to, town)) = take_dispatched(node, resident, job) else {
        return false;
    };
    let acting = match crate::authorization::resident(node, resident) {
        Ok(acting) => acting,
        Err(failure) => {
            node.log.line(format_args!("removing a completed job: {failure}"));
            return true;
        }
    };
    let (node, call) = (
        node.clone(),
        RemoteCall::Forget {
            resident: resident.clone(),
            town,
            job: job.clone(),
            delivery: None,
            origin: None,
            through_exit: false,
        },
    );
    tokio::spawn(async move {
        match node.call(&acting, &to, Purpose::Hands, call).await {
            Ok(Ok(_)) => {}
            Ok(Err(failure)) => {
                node.log.line(format_args!("removing a job shown whole on {}: {}", to.name, failure.message))
            }
            Err(failure) => node.log.line(format_args!("removing a job shown whole on {}: {failure}", to.name)),
        }
    });
    true
}

/// A run here for `resident` of `from`, handed here whole, for `account`.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn serve(
    node: &Arc<Node>,
    from: MachineName,
    account: HubAccount,
    resident: ResidentId,
    town: Option<TownName>,
    request: RunRequest,
    answer_receipts: bool,
    dispatch: Option<fastctx_types::NoticeId>,
) -> Result<RemoteReply, Failure> {
    let owner = Address { machine: from, resident };
    let (reply, _) = node.jobs.run(node, owner.into(), town, request, Some(account), answer_receipts, dispatch).await?;
    Ok(RemoteReply::Run(Box::new(reply)))
}

/// Words came for `resident` of `from`: its runs waiting here answer.
pub(crate) fn words_came(node: &Node, from: MachineName, resident: ResidentId) -> RemoteReply {
    node.words.raise(&Address { machine: from, resident });
    RemoteReply::Set
}

/// A turn of `resident` of `from` was interrupted: a run of it here stops
/// the job it waits for.
pub(crate) fn turn_interrupted(node: &Node, from: MachineName, resident: ResidentId) -> RemoteReply {
    node.interrupts.raise(&Address { machine: from, resident });
    RemoteReply::Set
}

/// The answer to a run here of `resident` of `from` showed its ended job
/// whole, and its host has it.
pub(crate) async fn forget_here(
    node: &Arc<Node>,
    from: MachineName,
    resident: ResidentId,
    town: Option<TownName>,
    job: JobId,
) -> RemoteReply {
    crate::jobs::forget(node, town, Address { machine: from, resident }, job).await;
    RemoteReply::Set
}
