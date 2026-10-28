//! `on` (`world-run.md`, 去别的机器): part of a script sent to another
//! machine, and the parts other machines send here.
//!
//! The sender collects everything first and sends it whole; the machine
//! that runs it starts an ordinary job only once the package is all there
//! and checks out. The job writes its output to that machine's disk, and
//! the sender follows it from byte offsets, so a connection that breaks is
//! taken up again where it broke and the job never depends on the sender
//! being there. The job's end goes to whoever follows it; when nobody does
//! at its end, it goes to the resident in a letter. Only the `on` that sent
//! it leaving on purpose stops it.

use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use fastctx_net::data::Stream;
use fastctx_net::link::{CallFailure, Feed};
use fastctx_types::access::{Acting, Purpose};
use fastctx_types::ipc::Failure;
use fastctx_types::on::{
    OnEnd, OnOrigin, OnPackage, OnPiece, OnRequest, OnSizes, OnSource, OnWord, PipeWord, SILENT_FOR, STILL_EVERY, TAKEN,
};
use fastctx_types::remote::{RemoteCall, RemoteReply};
use fastctx_types::residents::Action;
use fastctx_types::run::{EndedHow, JobOwner, JobRecord, OnJob, RunRequest};
use fastctx_types::{JobId, MachineName};
use sha2::{Digest as _, Sha256};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::Instant;

use crate::Node;

/// The most bytes read from a log and handed over at once: one data
/// message (`fastctx_net::wire::MAX_DATA_MESSAGE`).
const CHUNK: usize = fastctx_net::wire::MAX_DATA_MESSAGE;

/// One queued chunk overlaps the IPC writer with the next network read.
/// Output waiting for that writer is at most 4 * ceil(CHUNK / 3) bytes;
/// the producer reserves this slot before receiving or encoding a chunk.
const QUEUED_PIECES: usize = 1;

/// How long a follower waits before looking at a quiet log again. Output
/// growing says nothing to anyone, so it is looked for; a tenth of a second
/// is below what a reader notices, and a look at a file's end costs little.
pub(crate) const LOOK_AGAIN: Duration = Duration::from_millis(100);

const WAITING: &str = "waiting for the CLI receipt";

pub(crate) enum Piece {
    Value(Result<OnPiece, Failure>),
    End { end: OnEnd, taken: oneshot::Sender<()> },
}

#[derive(Clone)]
struct Pieces(mpsc::Sender<Piece>, watch::Receiver<bool>, Arc<Mutex<ReceiptContext>>);

#[derive(Clone)]
struct ReceiptContext {
    node: Arc<Node>,
    parent: Option<PathBuf>,
    origin: OnOrigin,
    destination: crate::reach::Written,
    scratch_hub: Option<fastctx_types::access::HubId>,
    acting: Acting,
    job: Option<JobId>,
    consumed: Vec<fastctx_types::NoticeId>,
    through_exit: bool,
    /// The step of the run that gave it, told as it goes.
    steps: Option<crate::on_steps::Steps>,
}

pub(crate) struct OnFollowing {
    pub(crate) pieces: mpsc::Receiver<Piece>,
    pub(crate) stop: watch::Sender<bool>,
}

impl Pieces {
    fn receipt(&self, consumed: Vec<fastctx_types::NoticeId>, through_exit: bool) -> Self {
        let mut context = self.2.lock().expect("never poisoned").clone();
        context.consumed = consumed;
        context.through_exit = through_exit;
        Self(self.0.clone(), self.1.clone(), Arc::new(Mutex::new(context)))
    }

    fn scratch(&self, mut locations: Vec<fastctx_types::scratch::Location>) -> Result<(), String> {
        if locations.is_empty() {
            return Ok(());
        }
        let context = self.2.lock().expect("never poisoned").clone();
        for location in &mut locations {
            if location.hub.is_none() {
                location.hub = context.scratch_hub.clone();
                location.machine = context.destination.name.clone();
            }
        }
        if let Some(parent) = &context.parent {
            crate::scratch::jobs::add(parent, locations.clone()).map_err(|error| error.to_string())?;
        }
        if context.node.is_me(&context.origin.resident.machine) {
            crate::scratch::observed(
                &context.node,
                &context.origin.resident.resident,
                &context.destination,
                &locations,
            )
            .map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    fn fact(&self, fact: &fastctx_types::run_facts::WriteFact) -> Result<(), String> {
        let (node, parent) = {
            let context = self.2.lock().expect("never poisoned");
            (context.node.clone(), context.parent.clone())
        };
        if let Some(record) = parent {
            crate::run_facts::write(&record.with_extension("writes"), fact).map_err(|error| error.to_string())?;
            crate::run_facts::update_job(&node, &record, vec![fact.clone()]).map_err(|error| error.to_string())?;
        }
        Ok(())
    }
    async fn send(&self, piece: Result<OnPiece, Failure>) -> Result<(), String> {
        if let Ok(OnPiece::Started { job, .. }) = &piece {
            self.2.lock().expect("never poisoned").job = Some(job.clone());
        }
        if let Some(steps) = self.steps() {
            let mut step = steps.lock().expect("never poisoned");
            match &piece {
                Ok(OnPiece::Started { job, .. }) => step.started(job),
                Err(failure) => step.failed(failure),
                Ok(_) => {}
            }
        }
        self.0.send(Piece::Value(piece)).await.map_err(|_| "the on that sent it left".to_string())
    }

    fn steps(&self) -> Option<crate::on_steps::Steps> {
        self.2.lock().expect("never poisoned").steps.clone()
    }

    /// Output on its way back, kept for the step's last lines.
    fn kept(&self, base64: &str) {
        if let Some(steps) = self.steps() {
            steps.lock().expect("never poisoned").output(base64);
        }
    }

    fn is_closed(&self) -> bool {
        self.0.is_closed()
    }

    fn stopping(&self) -> bool {
        *self.1.borrow()
    }

    async fn closed(&self) {
        self.0.closed().await;
    }

    async fn ended(&self, end: OnEnd) -> Result<bool, String> {
        // The step ends before the `on` that gave it is told, so the run
        // never ends ahead of it.
        if let Some(steps) = self.steps() {
            steps.lock().expect("never poisoned").ended(&end);
        }
        let (node, origin, consumed, through_exit) = {
            let context = self.2.lock().expect("never poisoned");
            (context.node.clone(), context.origin.clone(), context.consumed.clone(), context.through_exit)
        };
        let complete = !through_exit && !matches!(&end, OnEnd::Exited { remaining, .. } if *remaining > 0);
        if origin.sentinel.is_none() && node.is_me(&origin.resident.machine) && !consumed.is_empty() {
            let directory = node.world.conversation(&origin.resident.resident);
            tokio::task::spawn_blocking(move || crate::job_receipts::apply_consumed(&directory, &consumed))
                .await
                .map_err(|error| error.to_string())?
                .map_err(|error| error.to_string())?;
        }
        let (taken, receipt) = oneshot::channel();
        self.0.send(Piece::End { end, taken }).await.map_err(|_| "the on that sent it left".to_string())?;
        let received = receipt.await.is_ok();
        if received && origin.sentinel.is_none() {
            let context = self.2.lock().expect("never poisoned");
            let job = context.job.as_ref().expect("started before end");
            crate::job_receipts::on_taken(
                &context.node,
                context.parent.as_deref(),
                &context.origin,
                &context.destination,
                &context.acting,
                crate::job_receipts::OnCompletion { job, consumed: &context.consumed, through_exit: !complete },
            )
            .map_err(|error| error.to_string())?;
        }
        Ok(received)
    }
}

// The sending side.

/// Whom an `on` from `origin` works for (`world-network.md`, 一个登录，只在
/// 登录它的那台电脑上用): a hop another machine sent works for the account it
/// was sent for, and for nothing else this computer is logged in to; an
/// `on` of a resident here, for whom the resident works.
pub(crate) fn acting(node: &Node, origin: &OnOrigin) -> Acting {
    match &origin.account {
        Some(account) => Acting::one(account.hub.clone(), account.account.clone()),
        None if node.is_me(&origin.resident.machine) => {
            crate::here::acting(node, &origin.resident.resident).unwrap_or(Acting::Accounts { accounts: Vec::new() })
        }
        None => Acting::Accounts { accounts: Vec::new() },
    }
}

/// The account the job `job` here was sent for, when another machine sent
/// it: an `on` in its script works for that account.
pub(crate) fn sent_for(node: &Node, job: Option<&JobId>) -> Option<fastctx_types::access::HubAccount> {
    let path = crate::stop::find(&node.world, job?).ok()??;
    let record = fastctx_hands::run::record::read(&path).ok()?;
    // A hop sent here, or a run handed over to where its resident works.
    record.on.and_then(|on| on.account).or(record.account)
}

/// Sends `request` for the resident in `origin`. The job's pieces come on
/// the returned channel until its end or a failure; dropping the channel
/// before the end stops the job.
pub async fn send(node: &Arc<Node>, mut origin: OnOrigin, request: OnRequest) -> Result<OnFollowing, Failure> {
    if let Some(job) = &request.job {
        let path = node.world.owner(&node.names(), &origin.resident).job_record(job);
        let record = fastctx_hands::run::record::read(&path)
            .map_err(|error| Failure::new(format!("reading the on's job: {error}")))?;
        origin.call_id = record.call_id;
        origin.turn = record.turn;
    } else if node.is_me(&origin.resident.machine) {
        origin.turn = node.residents.now(&origin.resident.resident).and_then(|now| now.turn);
    }
    origin.request = Some(crate::speech::new_id("on"));
    origin.follower = Some(node.machine());
    let (pieces, receiver) = mpsc::channel(QUEUED_PIECES);
    let (stop, stopping) = watch::channel(false);
    let parent = request
        .job
        .as_ref()
        .map(|job| node.world.job_owner(&node.names(), &origin.resident.clone().into()).job_record(job));
    let steps = if origin.sentinel.is_none() { crate::on_steps::begin(node, parent.as_ref(), &request) } else { None };
    let context = ReceiptContext {
        scratch_hub: None,
        node: node.clone(),
        steps,
        parent,
        acting: acting(node, &origin),
        origin: origin.clone(),
        destination: request.written(),
        job: None,
        consumed: Vec::new(),
        through_exit: false,
    };
    let pieces = Pieces(pieces, stopping, Arc::new(Mutex::new(context)));
    let node = node.clone();
    tokio::spawn(async move {
        let waiting = Arc::new(Mutex::new(crate::on_delivery::Following::start(&node, &origin, &request.machine)));
        let local = node.is_here(&request.written());
        let sent = if local {
            here(&node, &origin, &request, &pieces, &waiting).await
        } else {
            there(&node, &origin, &request, &pieces, &waiting).await
        };
        if let Err(mut failure) = sent {
            // Publication can fail after making a prepared debt visible.
            // Once a refusal is handed out, it cannot later become an end.
            if failure.not_run
                && let Err(error) = crate::on_delivery::cancelled(&node, &origin, &request.machine)
            {
                failure = Failure::new(format!("keeping the startup refusal: {error}"));
            }
            if local && !failure.not_run && pieces.stopping() {
                crate::on_stop::send(&node, &request.written(), &origin, None).await;
            }
            let _ = pieces.send(Err(failure)).await;
        }
    });
    Ok(OnFollowing { pieces: receiver, stop })
}

/// What the sender knows of a job it sent, kept across the connections
/// that follow it.
#[derive(Default)]
struct Followed {
    job: Option<JobId>,
    refused: Option<Failure>,
    out: u64,
    err: u64,
    ending: Option<EndReceipt>,
}

type EndReceipt = watch::Receiver<Option<Result<bool, Failure>>>;

async fn end_received(receipt: &mut EndReceipt) -> Result<bool, Failure> {
    loop {
        if let Some(result) = receipt.borrow_and_update().clone() {
            return result;
        }
        receipt.changed().await.map_err(|_| Failure::new("the completion receipt task stopped"))?;
    }
}

fn offer_end(
    followed: &Arc<Mutex<Followed>>,
    pieces: &Pieces,
    waiting: &Arc<Mutex<crate::on_delivery::Following>>,
    end: OnEnd,
) -> EndReceipt {
    let mut state = followed.lock().expect("never poisoned");
    if let Some(receipt) = &state.ending {
        return receipt.clone();
    }
    let job = state.job.clone().expect("started before end");
    let (done, receipt) = watch::channel(None);
    state.ending = Some(receipt.clone());
    let (pieces, waiting) = (pieces.clone(), waiting.clone());
    // This handover outlives a network feed. A late control reply must not
    // cancel a completion already queued for the CLI, or cause a second one.
    tokio::spawn(async move {
        let complete = !pieces.2.lock().expect("never poisoned").through_exit
            && !matches!(&end, OnEnd::Exited { remaining, .. } if *remaining > 0);
        let result = match pieces.ended(end).await {
            Ok(true) if complete => waiting
                .lock()
                .expect("never poisoned")
                .taken(&job)
                .map(|()| true)
                .map_err(|error| Failure::new(format!("keeping the completion receipt: {error}"))),
            Ok(taken) => Ok(taken),
            Err(_) => Ok(false),
        };
        let _ = done.send(Some(result));
    });
    receipt
}

/// What the sender does with a job's id once the machine says it started,
/// before any of its output is handed on.
type OnStarted = Arc<dyn Fn(JobId) -> fastctx_net::link::BoxFuture<Result<(), String>> + Send + Sync>;

/// An `on` naming this machine: the same job, without the network.
async fn here(
    node: &Arc<Node>,
    origin: &OnOrigin,
    request: &OnRequest,
    pieces: &Pieces,
    waiting: &Arc<Mutex<crate::on_delivery::Following>>,
) -> Result<(), Failure> {
    node.authorize_here(&acting(node, origin))?;
    let staged = Staged::local(&request.package).await.map_err(|failure| Failure::not_run(failure.message))?;
    let (job, record_path, follower) = start(node, origin, staged, request.package.writes_off, false).await?;
    waiting.lock().expect("never poisoned").started(&job);
    noted(node, origin, request, &job).await?;
    let _ = pieces.send(Ok(OnPiece::Started { machine: node.machine(), job: job.clone() })).await;
    let mut sink = Sink::Local(pieces);
    let handed = hand_over(node, &job, &record_path, 0, 0, &mut sink).await;
    if matches!(handed, Ok(Handed { taken: true, complete: true })) {
        waiting
            .lock()
            .expect("never poisoned")
            .taken(&job)
            .map_err(|error| Failure::new(format!("keeping the completion receipt: {error}")))?;
    }
    let stopped = if pieces.stopping() && !matches!(handed, Ok(Handed { taken: true, .. })) {
        crate::on_stop::send(node, &request.written(), origin, Some(&job)).await;
        waiting
            .lock()
            .expect("never poisoned")
            .taken(&job)
            .map_err(|error| Failure::new(format!("keeping the intentional stop: {error}")))
    } else {
        Ok(())
    };
    let finished = follower.finish(handed).await;
    stopped.and(finished)
}

/// An `on` naming another machine.
async fn there(
    node: &Arc<Node>,
    origin: &OnOrigin,
    request: &OnRequest,
    pieces: &Pieces,
    waiting: &Arc<Mutex<crate::on_delivery::Following>>,
) -> Result<(), Failure> {
    let machine = request.machine.clone();
    let written = request.written();
    let acting = acting(node, origin);
    let (link, account) =
        node.speaking(&acting, &written, Purpose::Hands).map_err(|failure| Failure::not_run(failure.to_string()))?;
    // Recorded here under its project; there, under it only on its hub.
    let local_origin = origin;
    let origin = &OnOrigin { town: node.town_across(&link, origin.town.as_ref())?, ..origin.clone() };
    {
        let mut context = pieces.2.lock().expect("never poisoned");
        context.origin = origin.clone();
        context.scratch_hub = Some(link.membership().id);
    }
    if origin.sentinel.is_some() {
        match link
            .call(&machine, &account, Purpose::Look, RemoteCall::Facts)
            .await
            .map_err(crate::authorization::call_failure)??
        {
            RemoteReply::Facts(facts) if facts.sentinel_look => {}
            _ => return Err(Failure::not_run("upgrade the observed machine to run sentinel observations")),
        }
    }
    let sizes = sizes(&request.package).await.map_err(|failure| Failure::not_run(failure.message))?;
    requested(node, local_origin, request).await?;
    let followed = Arc::new(Mutex::new(Followed::default()));
    // Noted when the machine says the job started, before its output is
    // handed on: the script's answer may be read as soon as the job is through.
    let started: OnStarted = {
        let (node, origin, request, waiting) = (node.clone(), local_origin.clone(), request.clone(), waiting.clone());
        Arc::new(move |job| {
            let (node, origin, request) = (node.clone(), origin.clone(), request.clone());
            waiting.lock().expect("never poisoned").started(&job);
            Box::pin(async move { noted(&node, &origin, &request, &job).await.map_err(|failure| failure.message) })
        })
    };
    // Its output goes straight to the `on` the script pipes it into when
    // that machine may take it from there (`world-run.md`, 数据跟着命令走).
    let piped = request.pipe_to.as_ref().is_some_and(|to| straight(node, &link, &account, &machine, to));
    // A sentinel's look is no script of the conversation.
    let attachments = match origin.sentinel {
        None => crate::attachments::follow::known(node, &origin.resident),
        Some(_) => Vec::new(),
    };
    let call = RemoteCall::On {
        origin: origin.clone(),
        sizes,
        writes_off: request.package.writes_off,
        piped,
        source: request.package.source.clone(),
        attachments,
    };
    let piping = origin.request.clone().filter(|_| piped);
    let sending = feed(
        &machine,
        Some(request.package.clone()),
        followed.clone(),
        pieces.clone(),
        Some(started),
        waiting.clone(),
        piping,
    );
    let first = link.call_with_stream(&machine, &account, Purpose::Hands, call, sending).await;
    let (started, refused) = {
        let followed = followed.lock().expect("never poisoned");
        (followed.job.clone(), followed.refused.clone())
    };
    if let Some(failure) = refused {
        return Err(failure);
    }
    let Some(job) = started else {
        let not_sent = matches!(
            &first,
            Err(CallFailure::NotConnected { .. }
                | CallFailure::NotMember { .. }
                | CallFailure::Offline { .. }
                | CallFailure::Refused { .. }
                | CallFailure::NotAllowed { .. }
                | CallFailure::Unreachable { .. })
        ) || matches!(&first, Ok(Err(failure)) if failure.not_run);
        if pieces.stopping() && !not_sent {
            crate::on_stop::send(node, &written, origin, None).await;
            return Ok(());
        }
        return Err(match first {
            Ok(Err(failure)) => failure,
            Ok(Ok(_)) => {
                Failure::new(format!("{machine} answered without a startup confirmation; execution is unknown"))
            }
            // Nothing ran there: the call did not get through.
            Err(
                failure @ (CallFailure::NotConnected { .. }
                | CallFailure::NotMember { .. }
                | CallFailure::Offline { .. }
                | CallFailure::Refused { .. }
                | CallFailure::NotAllowed { .. }
                | CallFailure::Unreachable { .. }),
            ) => Failure::not_run(failure.to_string()),
            Err(failure) => Failure::new(format!(
                "{failure}. If the job started there, its end will be told to you; `fastctx w` on {machine} shows it"
            )),
        });
    };
    let followed_through =
        follow_there(node, (&link, &account), origin, &written, &job, &followed, pieces, waiting).await;
    if matches!(followed_through, Ok(true)) {
        waiting
            .lock()
            .expect("never poisoned")
            .taken(&job)
            .map_err(|error| Failure::new(format!("keeping the intentional stop: {error}")))?;
    }
    followed_through.map(|_| ())
}

/// Whether the output of an `on` to `from` may go straight to `to`, the
/// machine of the `on` the script pipes it into: neither is this machine,
/// through which it would come anyway, and `to` is a machine of `link`'s
/// hub, where `from` is, that `account` works on and that may act for it,
/// as taking the output from `from` asks. A machine written `name.hub` of
/// another hub is never reached by a stream of this one.
fn straight(
    node: &Node,
    link: &fastctx_net::link::Link,
    account: &fastctx_types::AccountName,
    from: &MachineName,
    to: &crate::reach::Written,
) -> bool {
    if to.hub.as_ref().is_some_and(|hub| *hub != link.membership().hub_name) {
        return false;
    }
    let view = link.view();
    let to = &to.name;
    !node.is_me(from)
        && !node.is_me(to)
        && view.member(to).is_some()
        && view.rights(account, to).hands
        && view.may_act_for(account, to)
}

/// Follows a job that started on `machine` until its end is taken, it is
/// stopped, or the World says it cannot be followed. True when the `on`
/// that sent it left, and so stopped it.
/// It is followed through the hub, and for the account, it was sent with.
#[allow(clippy::too_many_arguments)]
async fn follow_there(
    node: &Arc<Node>,
    (link, account): (&Arc<fastctx_net::link::Link>, &fastctx_types::AccountName),
    origin: &OnOrigin,
    written: &crate::reach::Written,
    job: &JobId,
    followed: &Arc<Mutex<Followed>>,
    pieces: &Pieces,
    waiting: &Arc<Mutex<crate::on_delivery::Following>>,
) -> Result<bool, Failure> {
    let machine = &written.name;
    let mut again = crate::again::Again::default();
    loop {
        if let Some(failure) = followed.lock().expect("never poisoned").refused.clone() {
            return Err(failure);
        }
        let ending = followed.lock().expect("never poisoned").ending.clone();
        if let Some(mut receipt) = ending
            && end_received(&mut receipt).await?
        {
            return Ok(false);
        }
        if pieces.is_closed() {
            if pieces.stopping() {
                crate::on_stop::send(node, written, origin, Some(job)).await;
                return Ok(true);
            }
            return Err(Failure::new("the local connection to the on follower broke; its end remains owed"));
        }
        tokio::select! {
            () = again.pause(node, std::slice::from_ref(written)) => {}
            () = pieces.closed() => continue,
        }
        let (out_from, err_from) = {
            let followed = followed.lock().expect("never poisoned");
            (followed.out, followed.err)
        };
        let call = RemoteCall::Follow { origin: origin.clone(), job: job.clone(), out_from, err_from };
        let following = feed(machine, None, followed.clone(), pieces.clone(), None, waiting.clone(), None);
        let result = link.call_with_stream(machine, account, Purpose::Hands, call, following).await;
        let progressed = {
            let followed = followed.lock().expect("never poisoned");
            followed.out != out_from || followed.err != err_from
        };
        if progressed {
            again.reset();
        }
        // A control reply can fail after the data stream already offered
        // the end. Only that handover's CLI receipt decides its outcome.
        if followed.lock().expect("never poisoned").ending.is_some() {
            continue;
        }
        match result {
            Ok(Err(failure)) => return Err(Failure::new(format!("following {job} on {machine}: {failure}"))),
            Err(CallFailure::NotMember { name, .. }) => {
                return Err(Failure::new(format!(
                    "{name} is no longer one of the user's machines; {job} was left there"
                )));
            }
            Ok(Ok(_)) | Err(_) => {}
        }
    }
}

/// The source end of an `On` or `Follow` connection: the package first, if
/// any, then what the job writes, until its end. A job whose output went
/// straight on, as dispatch `piping`, is told to `fastctx on` once started.
fn feed(
    machine: &MachineName,
    package: Option<OnPackage>,
    followed: Arc<Mutex<Followed>>,
    pieces: Pieces,
    started: Option<OnStarted>,
    waiting: Arc<Mutex<crate::on_delivery::Following>>,
    piping: Option<fastctx_types::NoticeId>,
) -> Feed {
    let machine = machine.clone();
    Box::new(move |mut stream: Stream| {
        Box::pin(async move {
            if let Some(package) = package {
                send_package(&mut stream, &package).await?;
            }
            loop {
                let word = tokio::select! {
                    word = tokio::time::timeout(SILENT_FOR, stream.hear::<OnWord>()) => match word {
                        Ok(word) => word?,
                        Err(_) => return Err(format!("{machine} said nothing for {}s", SILENT_FOR.as_secs())),
                    },
                    () = pieces.closed() => return Err("the on that sent it left".to_string()),
                };
                match word {
                    OnWord::WriteFact { fact } => pieces.fact(&fact)?,
                    OnWord::Refused { failure } => {
                        followed.lock().expect("never poisoned").refused = Some(failure);
                        return stream.say(&TAKEN).await;
                    }
                    OnWord::Started { job, scratch } => {
                        pieces.scratch(vec![fastctx_types::scratch::Location {
                            hub: None,
                            machine: machine.clone(),
                            path: scratch.ok_or_else(|| fastctx_types::scratch::words::OLD_NODE.into()),
                        }])?;
                        followed.lock().expect("never poisoned").job = Some(job.clone());
                        if let Some(started) = &started
                            && let Err(error) = started(job.clone()).await
                        {
                            followed.lock().expect("never poisoned").refused = Some(Failure::new(error.clone()));
                            return Err(error);
                        }
                        let _ = pieces.send(Ok(OnPiece::Started { machine: machine.clone(), job: job.clone() })).await;
                        if let Some(request) = &piping {
                            let source = OnSource { machine: machine.clone(), job, request: request.clone() };
                            let _ = pieces.send(Ok(OnPiece::Piped { source })).await;
                        }
                    }
                    OnWord::Out { mut bytes } | OnWord::Err { mut bytes } => {
                        let error = matches!(word, OnWord::Err { .. });
                        while bytes > 0 {
                            // Waiting for the CLI is not a silent network. Reserve
                            // outside SILENT_FOR, before reading any more output.
                            let slot = pieces.0.reserve().await.map_err(|_| "the on that sent it left".to_string())?;
                            let count = bytes.min(CHUNK as u64);
                            let mut taken = Vec::with_capacity(count as usize);
                            stream
                                .take(count, |chunk| {
                                    taken.extend_from_slice(chunk);
                                    Ok(())
                                })
                                .await?;
                            let base64 = STANDARD.encode(&taken);
                            pieces.kept(&base64);
                            let piece = if error { OnPiece::Err { base64 } } else { OnPiece::Out { base64 } };
                            // A cancelled feed must either leave both unchanged
                            // or retain the bytes and advance their resume offset.
                            // No await may separate these two operations.
                            let mut followed = followed.lock().expect("never poisoned");
                            slot.send(Piece::Value(Ok(piece)));
                            if error {
                                followed.err += count
                            } else {
                                followed.out += count
                            }
                            bytes -= count;
                        }
                    }
                    OnWord::Still { scratch } => {
                        pieces.scratch(scratch)?;
                    }
                    OnWord::Ended { end, consumed, through_exit, scratch, .. } => {
                        pieces.scratch(scratch)?;
                        let handed = pieces.receipt(consumed, through_exit);
                        let mut receipt = offer_end(&followed, &handed, &waiting, end);
                        let taken = loop {
                            tokio::select! {
                                result = end_received(&mut receipt) => break result.map_err(|failure| failure.message)?,
                                () = tokio::time::sleep(STILL_EVERY) => stream.say(&WAITING).await?,
                            }
                        };
                        if !taken {
                            return Err("the on that sent it left".to_string());
                        }
                        return stream.say(&TAKEN).await;
                    }
                }
            }
        })
    })
}

/// Sends the package's parts in the order `OnSizes` lists them.
async fn send_package(stream: &mut Stream, package: &OnPackage) -> Result<(), String> {
    for path in [Some(&package.state), Some(&package.script), package.input.as_ref()].into_iter().flatten() {
        let mut file = tokio::fs::File::open(path).await.map_err(|error| format!("reading {path}: {error}"))?;
        let mut buffer = vec![0u8; CHUNK];
        loop {
            let read = tokio::io::AsyncReadExt::read(&mut file, &mut buffer)
                .await
                .map_err(|error| format!("reading {path}: {error}"))?;
            if read == 0 {
                break;
            }
            stream.send(&buffer[..read]).await?;
        }
    }
    Ok(())
}

/// The package's sizes and digest, read once before anything is sent.
async fn sizes(package: &OnPackage) -> Result<OnSizes, Failure> {
    let package = package.clone();
    tokio::task::spawn_blocking(move || -> std::io::Result<OnSizes> {
        let mut digest = Sha256::new();
        let mut size = |path: Option<&String>| -> std::io::Result<Option<u64>> {
            let Some(path) = path else { return Ok(None) };
            let mut file = std::fs::File::open(path)?;
            let mut buffer = vec![0u8; CHUNK];
            let mut total = 0u64;
            loop {
                let read = file.read(&mut buffer)?;
                if read == 0 {
                    return Ok(Some(total));
                }
                digest.update(&buffer[..read]);
                total += read as u64;
            }
        };
        let state = size(Some(&package.state))?.expect("given");
        let script = size(Some(&package.script))?.expect("given");
        let input = size(package.input.as_ref())?.unwrap_or(0);
        Ok(OnSizes { state, script, input, sha256: hex(&digest.finalize()) })
    })
    .await
    .map_err(|error| Failure::new(format!("reading the package: {error}")))?
    .map_err(|error| Failure::new(format!("reading the package: {error}")))
}

/// Records the `on` where it was written: in the resident's actions, when
/// it lives here, so the machine it went to may write back about it; and
/// beside the job whose script it is part of, for that job's answer.
async fn requested(node: &Arc<Node>, origin: &OnOrigin, request: &OnRequest) -> Result<(), Failure> {
    if origin.sentinel.is_some() || !node.is_me(&origin.resident.machine) {
        return Ok(());
    }
    let action = Action {
        turn: origin.turn.clone(),
        at: jiff::Timestamp::now(),
        machine: request.machine.clone(),
        hand: "on".to_string(),
        what: "sending a script".to_string(),
        result: String::new(),
    };
    let (world, origin) = (node.world.clone(), origin.clone());
    tokio::task::spawn_blocking(move || {
        let directory = world.conversation(&origin.resident.resident);
        let _transaction = crate::resident_storage::lock();
        let mut bytes = match std::fs::read(directory.actions()) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(error) => return Err(error),
        };
        // A prior append may have failed mid-line. Preserve that fragment,
        // but do not let it swallow the durable action for this request.
        if !bytes.is_empty() && !bytes.ends_with(b"\n") {
            bytes.push(b'\n');
        }
        bytes.extend(serde_json::to_vec(&action).map_err(std::io::Error::other)?);
        bytes.push(b'\n');
        fastctx_platform::publish(&directory.actions(), &bytes, fastctx_platform::Permissions::Private)
    })
    .await
    .map_err(|error| Failure::not_run(format!("recording the request before sending it: {error}")))?
    .map_err(|error| Failure::not_run(format!("recording the request before sending it: {error}")))
}

async fn noted(node: &Arc<Node>, origin: &OnOrigin, request: &OnRequest, job: &JobId) -> Result<(), Failure> {
    if origin.sentinel.is_some() {
        return Ok(());
    }
    let here = node.names();
    let action = Action {
        turn: origin.turn.clone(),
        at: jiff::Timestamp::now(),
        machine: request.machine.clone(),
        hand: "on".to_string(),
        what: job.to_string(),
        result: String::new(),
    };
    let (world, origin, request, job) = (node.world.clone(), origin.clone(), request.clone(), job.clone());
    tokio::task::spawn_blocking(move || -> std::io::Result<()> {
        let owner = JobOwner::Resident(origin.resident.clone());
        if fastctx_types::layout::Here::is_here(&here, &origin.resident.machine) {
            crate::residents::record_action(&world, &origin.resident.resident, &action)?;
        }
        if let Some(parent) = &request.job {
            let directory = world.job_owner(&here, &owner);
            let line = format!("{} {job}\n", request.machine);
            append(&directory.jobs().join(format!("{parent}.on")), line.as_bytes())?;
        }
        Ok(())
    })
    .await
    .map_err(|error| Failure::new(format!("recording the started job: {error}")))?
    .map_err(|error| Failure::new(format!("recording the started job: {error}")))
}

fn append(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    std::fs::OpenOptions::new().create(true).append(true).open(path)?.write_all(bytes)
}

// The receiving side.

/// How an `On` call asks for its job: with the World's writes off, its
/// output going straight on, its input another `on`'s output.
pub(crate) struct Sent {
    pub(crate) writes_off: bool,
    pub(crate) piped: bool,
    pub(crate) source: Option<OnSource>,
    pub(crate) attachments: Vec<fastctx_types::attachments::Attachment>,
}

/// Runs the package an `On` call from `from` brings, and hands over the
/// job's output on `stream` until its end.
pub async fn serve_on(
    node: &Arc<Node>,
    origin: OnOrigin,
    sizes: OnSizes,
    how: Sent,
    mut stream: Stream,
) -> Result<RemoteReply, Failure> {
    let starting = async {
        if origin.sentinel.is_some()
            && (sizes.state != 0 || sizes.input != 0 || sizes.script > 8192 || how.piped || how.source.is_some())
        {
            return Err(Failure::not_run("invalid internal observation package"));
        }
        let mut staged = Staged::receive(node, &origin, &sizes, &mut stream)
            .await
            .map_err(|failure| Failure::not_run(failure.message))?;
        if let Some(source) = &how.source {
            let path = staged.state.with_extension("in");
            take_piped(node, &origin, source, &path, &mut stream).await?;
            staged.input = Some(path);
        }
        if origin.sentinel.is_none() {
            bring_files(node, &origin, how.attachments, &mut stream).await?;
        }
        start(node, &origin, staged, how.writes_off, how.piped).await
    }
    .await;
    let (job, record_path, follower) = match starting {
        Ok(started) => started,
        Err(failure) if !failure.not_run => return Err(failure),
        Err(failure) => {
            // Closing the data stream first races the control reply and
            // turns a definite refusal into an unknown execution outcome.
            // Hand over the refusal on the stream before answering the call.
            stream
                .say(&OnWord::Refused { failure: failure.clone() })
                .await
                .map_err(|error| Failure::new(format!("saying the job was not started: {error}")))?;
            match tokio::time::timeout(SILENT_FOR, stream.hear::<String>()).await {
                Ok(Ok(word)) if word == TAKEN => return Err(failure),
                Ok(Ok(_)) => return Err(Failure::new("the sender did not acknowledge the refusal")),
                Ok(Err(error)) => return Err(Failure::new(error)),
                Err(_) => {
                    return Err(Failure::new("the sender did not acknowledge the refusal before its line went silent"));
                }
            }
        }
    };
    let handed = async {
        let room = fastctx_hands::run::supervisor::room(&record_path).map_err(|error| error.to_string())?;
        stream
            .say(&OnWord::Started {
                job: job.clone(),
                scratch: Some(fastctx_hands::run::present::display_path(&room.scratch().to_string_lossy())),
            })
            .await?;
        let mut sink = Sink::Stream(&mut stream);
        hand_over(node, &job, &record_path, 0, 0, &mut sink).await
    }
    .await;
    follower.finish(handed).await?;
    Ok(RemoteReply::OnDone)
}

/// Brings the files the user attached in the conversation into its scratch
/// here before its job starts, saying `Still` to whoever sent it while they
/// come. A file not to be had leaves the job to start without it.
async fn bring_files(
    node: &Arc<Node>,
    origin: &OnOrigin,
    files: Vec<fastctx_types::attachments::Attachment>,
    stream: &mut Stream,
) -> Result<(), Failure> {
    if files.is_empty() {
        return Ok(());
    }
    let acting = acting(node, origin);
    let mut bringing = Box::pin(crate::attachments::follow::bring(node, &origin.resident, &acting, files, None));
    loop {
        tokio::select! {
            _ = &mut bringing => return Ok(()),
            () = tokio::time::sleep(STILL_EVERY) => {
                stream
                    .say(&OnWord::Still { scratch: Vec::new() })
                    .await
                    .map_err(|error| Failure::new(format!("telling the sender its files are coming: {error}")))?;
            }
        }
    }
}

/// Takes the output of the `on` `source` names, which went straight on to
/// this one, as the input of the job about to start here, saying `Still` to
/// whoever sent this one while it comes, so the wait is not taken for a
/// broken line. Nothing has run here when it does not come.
async fn take_piped(
    node: &Arc<Node>,
    origin: &OnOrigin,
    source: &OnSource,
    path: &Path,
    stream: &mut Stream,
) -> Result<(), Failure> {
    let producer = OnOrigin { request: Some(source.request.clone()), ..origin.clone() };
    let mut taking = Box::pin(take_output(node, &producer, source, path));
    loop {
        tokio::select! {
            taken = &mut taking => {
                return taken.map_err(|failure| {
                    Failure::not_run(format!(
                        "its input, the output of {} on {}, did not arrive: {}",
                        source.job, source.machine, failure.message
                    ))
                });
            }
            () = tokio::time::sleep(STILL_EVERY) => {
                stream
                    .say(&OnWord::Still { scratch: Vec::new() })
                    .await
                    .map_err(|error| Failure::new(format!("telling the sender its input is coming: {error}")))?;
            }
        }
    }
}

/// Takes the output of `source`'s job, sent by `producer`, into `path` once
/// it is all written: from this machine's own disk, or over a data
/// connection from the machine it was written on.
async fn take_output(node: &Arc<Node>, producer: &OnOrigin, source: &OnSource, path: &Path) -> Result<(), Failure> {
    if node.is_me(&source.machine) {
        let record_path = on_job(node, producer, &source.job)?;
        let _reading = crate::job_reader::open(node, &record_path).await?;
        piped_record(&record_path, producer).await?;
        let mut changes = node.jobs.subscribe();
        let record = loop {
            let record = crate::jobs::read(&record_path).await?;
            if through(&record).is_some() {
                break record;
            }
            tokio::select! {
                _ = changes.recv() => {}
                () = tokio::time::sleep(LOOK_AGAIN) => {}
            }
        };
        let through = record.output_drained.filter(|_| record.end.is_none()).map(|boundary| boundary.stdout);
        let (from, to) = (PathBuf::from(&record.output), path.to_path_buf());
        tokio::task::spawn_blocking(move || -> std::io::Result<()> {
            let file = std::fs::File::open(&from)?;
            let mut reader: Box<dyn std::io::Read> = match through {
                Some(limit) => Box::new(file.take(limit)),
                None => Box::new(file),
            };
            let mut writer = std::fs::File::create(&to)?;
            std::io::copy(&mut reader, &mut writer)?;
            writer.sync_all()
        })
        .await
        .map_err(|error| Failure::new(error.to_string()))?
        .map_err(|error| Failure::new(format!("taking the output: {error}")))?;
        piped_taken(node, &record_path);
        return Ok(());
    }
    let acting = acting(node, producer);
    let call = RemoteCall::PipeOut { origin: producer.clone(), job: source.job.clone() };
    let arrived: Arc<Mutex<bool>> = Arc::default();
    let from = source.machine.clone();
    let feed = || -> Feed {
        let (path, arrived, from) = (path.to_path_buf(), arrived.clone(), from.clone());
        Box::new(move |mut stream: Stream| {
            Box::pin(async move {
                use std::io::Write as _;
                let mut file =
                    std::fs::File::create(&path).map_err(|error| format!("writing {}: {error}", path.display()))?;
                let (mut taken, mut digest) = (0u64, Sha256::new());
                let work = stream.receipt_work();
                loop {
                    let word = tokio::time::timeout(SILENT_FOR, stream.hear::<PipeWord>())
                        .await
                        .map_err(|_| format!("{from} said nothing for {}s", SILENT_FOR.as_secs()))??;
                    match word {
                        PipeWord::Out { bytes } => {
                            let (back, written) = stream
                                .take_writing(bytes, (file, digest), |(file, digest), chunk| {
                                    digest.update(chunk);
                                    file.write_all(chunk).map_err(|error| error.to_string())
                                })
                                .await?;
                            (file, digest) = back;
                            written.map_err(|error| format!("writing {}: {error}", path.display()))?;
                            taken += bytes;
                        }
                        PipeWord::Still => {}
                        PipeWord::End { bytes, sha256 } => {
                            if bytes != taken || sha256 != hex(&digest.finalize()) {
                                return Err("the output arrived different from what was sent".to_string());
                            }
                            let settling = work.begin();
                            tokio::task::spawn_blocking(move || {
                                let _settling = settling;
                                file.sync_all()
                            })
                            .await
                            .map_err(|error| error.to_string())?
                            .map_err(|error| format!("writing {}: {error}", path.display()))?;
                            *arrived.lock().expect("never poisoned") = true;
                            return stream.done().await;
                        }
                    }
                }
            })
        })
    };
    let answer = Box::pin(node.call_with_stream(&acting, &(&source.machine).into(), Purpose::Hands, call, feed)).await;
    match answer {
        Ok(Ok(_)) if *arrived.lock().expect("never poisoned") => Ok(()),
        Ok(Ok(_)) => Err(Failure::new(format!("{} answered before the output arrived", source.machine))),
        Ok(Err(failure)) => Err(failure),
        Err(failure) => Err(Failure::new(failure.to_string())),
    }
}

/// Refuses unless the job at `record_path` sent its output straight on, for
/// the account `origin` is for.
async fn piped_record(record_path: &Path, origin: &OnOrigin) -> Result<(), Failure> {
    let record = crate::jobs::read(record_path).await?;
    if record.on.as_ref().is_some_and(|on| on.piped && on.account == origin.account) {
        return Ok(());
    }
    Err(Failure::not_run(format!("{} did not send its output on for the account asking for it", record.id)))
}

/// Notes that the output of the job at `record_path` reached the `on` that
/// reads it; once its follower took its end too, the job's files go: the
/// output is there now (`world-run.md`, 输出).
fn piped_taken(node: &Arc<Node>, record_path: &Path) {
    let marker = record_path.with_extension("on-piped");
    if let Err(error) = fastctx_platform::publish(&marker, b"", fastctx_platform::Permissions::Private) {
        node.log.line(format_args!("noting that {} went on: {error}", record_path.display()));
        return;
    }
    if record_path.with_extension("on-taken").exists() {
        crate::job_watch::copied(node, record_path);
    }
}

/// Hands the standard output of `job`, an `on` job of `origin` whose output
/// went straight on, to the machine of the `on` that reads it, as it is
/// written and until it is all there.
pub async fn serve_pipe(
    node: &Arc<Node>,
    origin: OnOrigin,
    job: JobId,
    mut stream: Stream,
) -> Result<RemoteReply, Failure> {
    let record_path = on_job(node, &origin, &job)?;
    piped_record(&record_path, &origin).await?;
    let failed = |error: String| Failure::new(format!("handing over the output of {job}: {error}"));
    let mut changes = node.jobs.subscribe();
    let (mut sent, mut digest, mut quiet_since) = (0u64, Sha256::new(), Instant::now());
    loop {
        // The record first: whatever it says has ended had written its last
        // byte before, so reading the log after it misses nothing.
        let record = crate::jobs::read(&record_path).await?;
        let boundary = record.output_drained.filter(|_| record.end.is_none() && through(&record).is_some());
        if let Some(bytes) = read_from(&record.output, sent, boundary.map(|b| b.stdout)).await.map_err(failed)? {
            stream.say(&PipeWord::Out { bytes: bytes.len() as u64 }).await.map_err(failed)?;
            stream.send(&bytes).await.map_err(failed)?;
            digest.update(&bytes);
            sent += bytes.len() as u64;
            quiet_since = Instant::now();
            if boundary.is_none_or(|boundary| sent < boundary.stdout) {
                continue;
            }
        }
        if through(&record).is_some() {
            break;
        }
        if quiet_since.elapsed() >= STILL_EVERY {
            stream.say(&PipeWord::Still).await.map_err(failed)?;
            quiet_since = Instant::now();
        }
        tokio::select! {
            change = changes.recv() => {
                if let Ok(changed) = change && changed != job {
                    continue;
                }
            }
            () = tokio::time::sleep(LOOK_AGAIN) => {}
            () = stream.closed() => return Err(failed("the machine taking it left".to_string())),
        }
    }
    stream.say(&PipeWord::End { bytes: sent, sha256: hex(&digest.finalize()) }).await.map_err(failed)?;
    stream.finish().await.map_err(failed)?;
    piped_taken(node, &record_path);
    Ok(RemoteReply::OnDone)
}

/// Goes on handing over an `on` job's output from where a broken
/// connection left it.
pub async fn serve_follow(
    node: &Arc<Node>,
    origin: OnOrigin,
    job: JobId,
    out_from: u64,
    err_from: u64,
    mut stream: Stream,
) -> Result<RemoteReply, Failure> {
    let record_path = on_job(node, &origin, &job)?;
    let follower = Follower::new(node, &record_path).await?;
    let mut sink = Sink::Stream(&mut stream);
    let handed = hand_over(node, &job, &record_path, out_from, err_from, &mut sink).await;
    follower.finish(handed).await?;
    Ok(RemoteReply::OnDone)
}

/// The record of the `on` job `job` of the resident in `origin`.
pub(crate) fn on_job(node: &Arc<Node>, origin: &OnOrigin, job: &JobId) -> Result<PathBuf, Failure> {
    let owner = JobOwner::Resident(origin.resident.clone());
    let path = node.world.job_record(&node.names(), origin.town.as_ref(), &owner, job);
    match fastctx_hands::run::record::read(&path) {
        Ok(record)
            if record
                .on
                .as_ref()
                .and_then(|on| on.request.as_ref())
                .is_some_and(|request| origin.request.as_ref() != Some(request)) =>
        {
            Err(Failure::refused(format!("{job} belongs to a different on request")))
        }
        Ok(record)
            if record.on.as_ref().is_some_and(|on| {
                on.sentinel != origin.sentinel || (on.sentinel.is_some() && on.account != origin.account)
            }) =>
        {
            Err(Failure::refused("the observation identity or account does not match"))
        }
        Ok(record) if record.on.is_some() => Ok(path),
        Ok(_) => Err(Failure::refused(format!("{job} on {} was not sent by an on", node.machine()))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err(Failure::output_not_retained(format!(
            "on job {job} of {} is no longer retained on {}",
            origin.resident,
            node.machine()
        ))),
        Err(error) => {
            Err(Failure::new(format!("reading on job {job} of {} on {}: {error}", origin.resident, node.machine())))
        }
    }
}

/// A package on this machine, ready to become a job.
struct Staged {
    state: PathBuf,
    script: String,
    input: Option<PathBuf>,
}

impl Staged {
    /// The package `fastctx on` wrote here.
    async fn local(package: &OnPackage) -> Result<Self, Failure> {
        let script = tokio::fs::read_to_string(&package.script)
            .await
            .map_err(|error| Failure::new(format!("reading the script {}: {error}", package.script)))?;
        Ok(Self { state: PathBuf::from(&package.state), script, input: package.input.as_ref().map(PathBuf::from) })
    }

    /// The package an `On` call brings, written to the resident's directory
    /// here and checked against its digest before anything runs.
    async fn receive(
        node: &Arc<Node>,
        origin: &OnOrigin,
        sizes: &OnSizes,
        stream: &mut Stream,
    ) -> Result<Self, Failure> {
        let owner = JobOwner::Resident(origin.resident.clone());
        let directory = node.world.job_owner(&node.names(), &owner).out();
        let name = random_name().map_err(|error| Failure::new(format!("naming the package: {error}")))?;
        let created = directory.clone();
        tokio::task::spawn_blocking(move || std::fs::create_dir_all(&created))
            .await
            .map_err(|error| Failure::new(error.to_string()))?
            .map_err(|error| Failure::new(format!("making {}: {error}", directory.display())))?;
        let mut digest = Sha256::new();
        let state = directory.join(format!("on-{name}.state"));
        take_to(stream, sizes.state, &state, &mut digest).await?;
        let mut bytes = Vec::new();
        stream
            .take(sizes.script, |chunk| {
                digest.update(chunk);
                bytes.extend_from_slice(chunk);
                Ok(())
            })
            .await
            .map_err(|error| Failure::new(format!("receiving the script: {error}")))?;
        let script = String::from_utf8(bytes).map_err(|_| Failure::new("the script is not UTF-8"))?;
        let input = if sizes.input > 0 {
            let path = directory.join(format!("on-{name}.in"));
            take_to(stream, sizes.input, &path, &mut digest).await?;
            Some(path)
        } else {
            None
        };
        if hex(&digest.clone().finalize()) != sizes.sha256 {
            return Err(Failure::new("the package arrived different from what was sent; nothing ran"));
        }
        Ok(Self { state, script, input })
    }
}

async fn take_to(stream: &mut Stream, size: u64, path: &Path, digest: &mut Sha256) -> Result<(), Failure> {
    use std::io::Write as _;
    let mut file =
        std::fs::File::create(path).map_err(|error| Failure::new(format!("writing {}: {error}", path.display())))?;
    let mut written = Ok(());
    stream
        .take(size, |chunk| {
            digest.update(chunk);
            if written.is_ok() {
                written = file.write_all(chunk);
            }
            Ok(())
        })
        .await
        .map_err(|error| Failure::new(format!("receiving the package: {error}")))?;
    written.map_err(|error| Failure::new(format!("writing {}: {error}", path.display())))
}

/// Starts the job a package holds, and marks its end as owed to the
/// resident: the follower takes that over until it leaves.
async fn start(
    node: &Arc<Node>,
    origin: &OnOrigin,
    staged: Staged,
    writes_off: bool,
    piped: bool,
) -> Result<(JobId, PathBuf, Follower), Failure> {
    if origin.sentinel.is_some() {
        crate::sentinels::look::admit(node, origin).await?;
    }
    let script = staged.script;
    let on = OnJob {
        sentinel: origin.sentinel.clone(),
        state: staged.state.to_string_lossy().into_owned(),
        request: origin.request.clone(),
        follower: origin.follower.clone(),
        account: origin.account.clone(),
        piped,
    };
    let request = RunRequest {
        turn: origin.turn.clone(),
        call_id: origin.call_id.clone(),
        script,
        cwd: None,
        writes_off,
        wait: None,
        attachments: Vec::new(),
    };
    let owner = JobOwner::Resident(origin.resident.clone());
    let registration = Arc::new(Mutex::new(None));
    let launched = node
        .jobs
        .launch(
            node,
            owner,
            origin.town.clone(),
            request,
            Some((on, staged.input)),
            Some(registration.clone()),
            None,
            false,
            None,
        )
        .await?;
    let follower = registration.lock().expect("never poisoned").take().expect("registered before spawn");
    Ok((launched.job, launched.record, follower))
}

pub(crate) type Registration = Arc<Mutex<Option<Follower>>>;

pub(crate) struct Follower {
    node: Arc<Node>,
    path: PathBuf,
    _reader: crate::job_reader::Lease,
    internal: bool,
}

impl Follower {
    pub(crate) fn reserved(node: &Arc<Node>, path: &Path, internal: bool) -> Self {
        *node.following.lock().expect("never poisoned").entry(path.to_path_buf()).or_default() += 1;
        Self { node: node.clone(), path: path.to_path_buf(), _reader: crate::job_reader::hold(node, path), internal }
    }

    async fn new(node: &Arc<Node>, path: &Path) -> Result<Self, Failure> {
        let (node, path) = (node.clone(), path.to_path_buf());
        tokio::task::spawn_blocking(move || {
            let _gate = node.disk.lock();
            let record = fastctx_hands::run::record::read(&path).map_err(|error| Failure::new(error.to_string()))?;
            Ok(Self::reserved(&node, &path, record.is_sentinel()))
        })
        .await
        .map_err(|error| Failure::new(error.to_string()))?
    }

    async fn finish(self, handed: Result<Handed, String>) -> Result<(), Failure> {
        if self.internal {
            if matches!(handed, Ok(Handed { taken: true, complete: true })) {
                let record = fastctx_hands::run::record::read(&self.path).map_err(|e| Failure::new(e.to_string()))?;
                let receipt = fastctx_types::run::JobDelivery {
                    job: record.id,
                    output: record.output,
                    at: jiff::Timestamp::now(),
                    shown_whole: true,
                };
                fastctx_agent::state::save(&self.path.with_extension("delivered"), &receipt).map_err(Failure::new)?;
                self.node.disk_due.lock().expect("never poisoned").insert(self.path.clone());
                self.node.disk_changed.notify_one();
            }
            return Ok(());
        }
        // A shell's background children still owe their eventual end,
        // even if they finished while the shell's receipt was in flight.
        if matches!(handed, Ok(Handed { taken: true, complete: true })) {
            fastctx_platform::publish(
                &self.path.with_extension("on-taken"),
                b"",
                fastctx_platform::Permissions::Private,
            )
            .map_err(|error| Failure::new(format!("keeping the completion receipt: {error}")))?;
        }
        Ok(())
    }
}

impl Drop for Follower {
    fn drop(&mut self) {
        {
            let mut following = self.node.following.lock().expect("never poisoned");
            let count = following.get_mut(&self.path).expect("registered follower");
            *count -= 1;
            if *count == 0 {
                following.remove(&self.path);
            }
        }
        if !self.internal {
            crate::inbox::retry_job(&self.node, &self.path);
        }
    }
}

/// Whether nobody follows `job` now, so its end goes in a letter.
pub fn unfollowed(node: &Node, record: &Path) -> bool {
    !node.following.lock().expect("never poisoned").contains_key(record)
}

/// Where a job's output is handed: the network, or this machine's own
/// `on`.
enum Sink<'a> {
    Stream(&'a mut Stream),
    Local(&'a Pieces),
}

struct Handed {
    taken: bool,
    complete: bool,
}

impl Sink<'_> {
    async fn facts(&mut self, facts: Vec<fastctx_types::run_facts::WriteFact>) -> Result<(), String> {
        for fact in facts {
            match self {
                Sink::Stream(stream) => stream.say(&OnWord::WriteFact { fact: Box::new(fact) }).await?,
                Sink::Local(pieces) => pieces.fact(&fact)?,
            }
        }
        Ok(())
    }
    async fn bytes(&mut self, error: bool, bytes: &[u8]) -> Result<(), String> {
        match self {
            Sink::Stream(stream) => {
                let length = bytes.len() as u64;
                stream.say(&if error { OnWord::Err { bytes: length } } else { OnWord::Out { bytes: length } }).await?;
                stream.send(bytes).await
            }
            Sink::Local(pieces) => {
                let slot = pieces.0.reserve().await.map_err(|_| "the on that sent it left".to_string())?;
                let base64 = STANDARD.encode(bytes);
                pieces.kept(&base64);
                let piece = if error { OnPiece::Err { base64 } } else { OnPiece::Out { base64 } };
                slot.send(Piece::Value(Ok(piece)));
                Ok(())
            }
        }
    }

    async fn scratch(&mut self, scratch: Vec<fastctx_types::scratch::Location>) -> Result<(), String> {
        match self {
            Sink::Stream(stream) => stream.say(&OnWord::Still { scratch }).await,
            Sink::Local(pieces) => pieces.scratch(scratch),
        }
    }

    async fn still(&mut self) -> Result<(), String> {
        match self {
            Sink::Stream(stream) => stream.say(&OnWord::Still { scratch: Vec::new() }).await,
            Sink::Local(_) => Ok(()),
        }
    }

    /// Hands over the end; true when the other side says it has it.
    async fn ended(
        &mut self,
        end: OnEnd,
        lettered: bool,
        consumed: Vec<fastctx_types::NoticeId>,
        through_exit: bool,
        scratch: Vec<fastctx_types::scratch::Location>,
    ) -> Result<bool, String> {
        match self {
            Sink::Stream(stream) => {
                stream.say(&OnWord::Ended { end, lettered, consumed, through_exit, scratch }).await?;
                loop {
                    match tokio::time::timeout(SILENT_FOR, stream.hear::<String>()).await {
                        Ok(Ok(word)) if word == WAITING => continue,
                        Ok(Ok(word)) => return Ok(word == TAKEN),
                        Ok(Err(error)) => return Err(error),
                        Err(_) => return Ok(false),
                    }
                }
            }
            Sink::Local(pieces) => {
                pieces.scratch(scratch)?;
                pieces.receipt(consumed, through_exit).ended(end).await
            }
        }
    }

    async fn closed(&mut self) {
        match self {
            Sink::Stream(stream) => stream.closed().await,
            Sink::Local(pieces) => pieces.closed().await,
        }
    }
}

/// Hands over the output of the job at `record_path` from the given byte
/// offsets until the job is through, then its end. True when the end was
/// taken; an error when the connection broke first.
async fn hand_over(
    node: &Arc<Node>,
    job: &JobId,
    record_path: &Path,
    mut out: u64,
    mut err: u64,
    sink: &mut Sink<'_>,
) -> Result<Handed, String> {
    let mut changes = node.jobs.subscribe();
    let mut facts_cursor = 0;
    let mut known_scratch = Vec::new();
    let mut quiet_since = Instant::now();
    loop {
        // The record first: whatever it says has ended had written its
        // last byte before, so reading the logs after it misses nothing.
        let record = crate::jobs::read(record_path).await.map_err(|failure| failure.message)?;
        let scratch =
            crate::scratch::jobs::locations(record_path, &record.machine).map_err(|error| error.to_string())?;
        if known_scratch != scratch {
            sink.scratch(scratch.clone()).await?;
            known_scratch = scratch;
        }
        if !record.is_sentinel() {
            sink.facts(
                crate::run_facts::read_since(record_path, &mut facts_cursor).map_err(|error| error.to_string())?,
            )
            .await?;
        }
        let errors = record.stderr.clone();
        let boundary = record.output_drained.filter(|_| record.end.is_none() && through(&record).is_some());
        // Output that went straight on is taken from here by another
        // machine, not handed to the follower.
        let piped = record.on.as_ref().is_some_and(|on| on.piped);
        let mut sent = false;
        if !piped && let Some(bytes) = read_from(&record.output, out, boundary.map(|b| b.stdout)).await? {
            out += bytes.len() as u64;
            sink.bytes(false, &bytes).await?;
            sent = true;
        }
        if let Some(path) = &errors
            && let Some(bytes) = read_from(path, err, boundary.map(|b| b.stderr)).await?
        {
            err += bytes.len() as u64;
            sink.bytes(true, &bytes).await?;
            sent = true;
        }
        let drained = boundary.is_some_and(|b| (piped || out >= b.stdout) && err >= b.stderr);
        if sent && !drained {
            quiet_since = Instant::now();
            continue;
        }
        if let Some(end) = through(&record) {
            if record.is_sentinel() {
                return sink
                    .ended(end, false, Vec::new(), record.end.is_none(), known_scratch)
                    .await
                    .map(|taken| Handed { taken, complete: record.end.is_some() });
            }
            sink.facts(crate::run_facts::writes(record_path).map_err(|error| error.to_string())?).await?;
            // A letter goes only for an end nobody followed, and takes the
            // marker that it is owed: without one, the letter went.
            let owed = node.world.job_owner(&node.names(), &record.owner).job_owed(job);
            let lettered = record.end.is_some() && matches!(tokio::fs::try_exists(&owed).await, Ok(false));
            let complete = record.end.is_some();
            let (node, path) = (node.clone(), record_path.to_path_buf());
            let consumed =
                tokio::task::spawn_blocking(move || crate::job_receipts::snapshot_through(&node, &path, !complete))
                    .await
                    .map_err(|error| error.to_string())?
                    .map_err(|error| error.to_string())?;
            return sink
                .ended(end, lettered, consumed, !complete, known_scratch)
                .await
                .map(|taken| Handed { taken, complete });
        }
        if quiet_since.elapsed() >= STILL_EVERY {
            sink.still().await?;
            quiet_since = Instant::now();
        }
        tokio::select! {
            change = changes.recv() => {
                if let Ok(changed) = change && changed != *job {
                    continue;
                }
            }
            () = tokio::time::sleep(LOOK_AGAIN) => {}
            () = sink.closed() => return Err("the follower left".to_string()),
        }
    }
}

/// How the job stands for `on`, once its shell has exited and its output
/// has drained, or it ended otherwise: an `on` ends as a subshell does.
fn through(record: &JobRecord) -> Option<OnEnd> {
    let status = record.exit.map(|exit| exit.status);
    // Output the log could not take was never handed over: the `on` did not
    // carry the job through, whatever its code.
    if let Some(lost) = record.output_lost.as_ref().or(record.stderr_lost.as_ref())
        && (record.end.is_some()
            || (record.output_drained.is_some() && record.remaining.is_some_and(|remaining| remaining > 0)))
    {
        let error = format!("its output there stopped being kept after {} bytes: {}", lost.kept_bytes, lost.error);
        return Some(OnEnd::Otherwise { how: EndedHow::Failed { error }, status });
    }
    if let Some(end) = &record.end {
        return Some(match (&end.how, status) {
            (EndedHow::Finished, Some(status)) => OnEnd::Exited { status, remaining: 0 },
            (how, status) => OnEnd::Otherwise { how: how.clone(), status },
        });
    }
    match status {
        Some(status)
            if (record.output_drained.is_some() || record.output_ended_at.is_some())
                && record.remaining.is_some_and(|remaining| remaining > 0) =>
        {
            Some(OnEnd::Exited { status, remaining: record.remaining.expect("checked") })
        }
        _ => None,
    }
}

/// Up to `CHUNK` bytes of the file at `path` from `offset`, when there are
/// any.
async fn read_from(path: &str, offset: u64, through: Option<u64>) -> Result<Option<Vec<u8>>, String> {
    let path = PathBuf::from(path);
    tokio::task::spawn_blocking(move || -> std::io::Result<Option<Vec<u8>>> {
        let mut file = match std::fs::File::open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let end = through.unwrap_or(file.metadata()?.len());
        if end <= offset {
            return Ok(None);
        }
        file.seek(SeekFrom::Start(offset))?;
        let mut buffer = vec![0u8; (end - offset).min(CHUNK as u64) as usize];
        let read = file.read(&mut buffer)?;
        buffer.truncate(read);
        Ok((read > 0).then_some(buffer))
    })
    .await
    .map_err(|error| error.to_string())?
    .map_err(|error| format!("reading the output: {error}"))
}

fn random_name() -> std::io::Result<String> {
    let mut bytes = [0u8; 8];
    getrandom::fill(&mut bytes).map_err(std::io::Error::other)?;
    Ok(hex(&bytes))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
