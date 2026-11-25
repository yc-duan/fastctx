//! The guardian owns at most a serving process and one prepared successor.
//! Busy/idle is entirely the node's decision, communicated on its private pipe.

use fastctx_platform::rolling_log::RollingLog;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::{Receiver, SyncSender},
};

use anyhow::Context as _;

use fastctx_platform::service_channel::{ServiceCommand, ServiceEvent};

use super::super::Kind;
use super::account::{self, Identity};
use super::child::Child;
use super::config::Config;

pub(super) struct Instance;

pub(super) enum Event {
    Wake,
    Node(Arc<Instance>, ServiceEvent),
    Exited(Arc<Instance>, anyhow::Result<u32>),
    ProtocolFailed(Arc<Instance>, std::io::Error),
}

#[derive(Default)]
pub(super) struct Requests {
    pub stop: AtomicBool,
    pub shutdown: AtomicBool,
    pub refresh: AtomicBool,
    /// Its user asked it to take the newer World program (`super::adopt`).
    pub adopt: AtomicBool,
    /// Its user said the next stop is planned.
    pub planned: AtomicBool,
    /// Its user asked it to start the node anew (`super::restart`).
    pub restart: AtomicBool,
    /// A hub's guardian took a newer World program by itself and ends, to be
    /// started again on it at once (`super::adopt::offered`).
    pub renewed: AtomicBool,
    pub wake_queued: AtomicBool,
}

/// The first pause before a node that could not be prepared is tried again,
/// the longest, and how long it is tried before the host ends.
const UNPREPARED_FIRST: std::time::Duration = std::time::Duration::from_secs(1);
const UNPREPARED_MOST: std::time::Duration = std::time::Duration::from_secs(30);
const UNPREPARED_AT_MOST: std::time::Duration = std::time::Duration::from_secs(30 * 60);

struct Node {
    child: Child,
    prepared: bool,
    serving: bool,
    yielding: bool,
    cancel_requested: bool,
    yielded: bool,
}

impl Node {
    fn is(&self, instance: &Arc<Instance>) -> bool {
        Arc::ptr_eq(&self.child.instance, instance)
    }
}

pub(super) fn run(
    config: Config,
    requests: Arc<Requests>,
    events: Receiver<Event>,
    sender: SyncSender<Event>,
    ready: impl FnOnce() -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    if config.target.is_some() {
        account::enable_privileges()?;
    }
    let mut log = RollingLog::open(&config.root()?.join("guardian.log"))?;
    let mut active: Option<Node> = None;
    let mut candidate: Option<Node> = None;
    // Keep one explicit profile reference across crashes as well as handoffs.
    // Detached jobs may still use HKCU while the next node is being created.
    let mut profile_lease = None;
    let mut refresh = true;
    let mut ready = Some(ready);
    // What the guardian last said of a newer program it did not take, said
    // once rather than each time it is asked again.
    let mut refused: Option<String> = None;
    // A node that cannot be prepared with none running, as while Windows
    // shuts down or its logon service is still starting, is tried again here
    // after a growing pause, rather than at once by the service manager.
    let mut unprepared: Option<(std::time::Instant, std::time::Duration)> = None;
    let mut unprepared_said: Option<String> = None;
    // The nodes ended as their user asked (`super::restart`): their channel
    // closing and their exit are no failure, and say nothing.
    let mut ended_as_asked: Vec<Arc<Instance>> = Vec::new();
    let result = (|| {
        loop {
            if requests.stop.load(Ordering::SeqCst) || requests.shutdown.load(Ordering::SeqCst) {
                return Ok(());
            }
            if requests.refresh.swap(false, Ordering::SeqCst) {
                refresh = true;
            }
            if requests.restart.swap(false, Ordering::SeqCst) && config.kind == Kind::Node {
                line(&mut log, "starting the node anew, as its user asked");
                for node in [candidate.take(), active.take()].into_iter().flatten() {
                    ended_as_asked.push(node.child.instance.clone());
                    if let Err(error) = node.child.terminate() {
                        line(&mut log, &format!("ending process {}: {error:#}", node.child.pid));
                    }
                }
                match super::restart::end_holder(&config) {
                    Ok(Some(pid)) => line(&mut log, &format!("ended process {pid}, which held the World unkept")),
                    Ok(None) => {}
                    Err(error) => line(&mut log, &format!("ending what holds the World: {error:#}")),
                }
                unprepared = None;
                refresh = true;
            }
            if requests.adopt.swap(false, Ordering::SeqCst) {
                match super::adopt::adopt(&config) {
                    Ok(()) => {
                        refused = None;
                        line(&mut log, "took the newer World program its user asked for");
                    }
                    Err(error) => {
                        let said = format!("did not take the World program: {error:#}");
                        if refused.as_deref() != Some(said.as_str()) {
                            line(&mut log, &said);
                            refused = Some(said);
                        }
                    }
                }
            }
            if refresh && config.kind == Kind::Hub && super::adopt::offered(&config) {
                match super::adopt::adopt(&config) {
                    Ok(()) => {
                        line(&mut log, "took the newer World program by itself; starting again on it");
                        requests.renewed.store(true, Ordering::SeqCst);
                        return Ok(());
                    }
                    Err(error) => {
                        let said = format!("did not take the newer World program: {error:#}");
                        if refused.as_deref() != Some(said.as_str()) {
                            line(&mut log, &said);
                            refused = Some(said);
                        }
                    }
                }
            }
            if refresh || active.is_none() && candidate.is_none() {
                refresh = false;
                let reconcile = reconcile(&config, &mut active, &mut candidate, &sender, &mut log);
                match reconcile {
                    Ok(()) => {
                        unprepared = None;
                        unprepared_said = None;
                    }
                    Err(error) => {
                        // A failed successor must never take a working node
                        // down. Session changes and the independent periodic
                        // check retry it; without any node, this host tries
                        // again after a pause, and the service manager starts
                        // it anew once it has tried for `UNPREPARED_AT_MOST`.
                        let said = format!("cannot prepare a node: {error:#}");
                        if unprepared_said.as_deref() != Some(said.as_str()) {
                            line(&mut log, &said);
                            unprepared_said = Some(said);
                        }
                        if active.is_none() {
                            let (since, pause) = match unprepared {
                                Some((since, pause)) => (since, (pause * 2).min(UNPREPARED_MOST)),
                                None => (std::time::Instant::now(), UNPREPARED_FIRST),
                            };
                            if since.elapsed() >= UNPREPARED_AT_MOST {
                                return Err(error);
                            }
                            refresh = true;
                            unprepared = Some((since, pause));
                        }
                    }
                }
            }
            if profile_lease.is_none() {
                profile_lease = active.as_ref().or(candidate.as_ref()).and_then(|node| node.child.profile_lease());
            }
            advance(&mut active, &mut candidate, &mut log)?;
            if (active.is_some() || candidate.is_some())
                && let Some(ready) = ready.take()
            {
                ready()?;
            }
            let event = match unprepared.filter(|_| active.is_none() && candidate.is_none()) {
                Some((_, pause)) => match events.recv_timeout(pause) {
                    Ok(event) => event,
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                        return Err(anyhow::anyhow!("the guardian's events ended"));
                    }
                },
                None => events.recv()?,
            };
            match event {
                Event::Wake => {
                    requests.wake_queued.store(false, Ordering::SeqCst);
                    refresh = true;
                }
                Event::Exited(instance, _) if ended_as_asked.iter().any(|ended| Arc::ptr_eq(ended, &instance)) => {
                    ended_as_asked.retain(|ended| !Arc::ptr_eq(ended, &instance));
                }
                Event::ProtocolFailed(instance, _)
                    if ended_as_asked.iter().any(|ended| Arc::ptr_eq(ended, &instance)) => {}
                Event::Exited(instance, status) => {
                    line(
                        &mut log,
                        &format!(
                            "process exited: {}",
                            match &status {
                                Ok(code) => code.to_string(),
                                Err(error) => format!("{error:#}"),
                            }
                        ),
                    );
                    if active.as_ref().is_some_and(|node| node.is(&instance)) {
                        let old = active.take().expect("matched instance");
                        line(
                            &mut log,
                            if old.yielded {
                                "idle handoff completed"
                            } else {
                                "foreground process stopped; restoring it"
                            },
                        );
                        refresh = true;
                    } else if candidate.as_ref().is_some_and(|node| node.is(&instance)) {
                        candidate.take();
                        cancel_yield(&mut active, &mut log);
                        refresh = true;
                    }
                }
                Event::ProtocolFailed(instance, error) => {
                    line(&mut log, &format!("node protocol failed: {error}"));
                    if let Some(node) = active.as_ref().filter(|node| node.is(&instance)) {
                        node.child.terminate()?;
                    }
                    if let Some(node) = candidate.as_ref().filter(|node| node.is(&instance)) {
                        node.child.terminate()?;
                    }
                }
                Event::Node(instance, event) => {
                    let is_active = active.as_ref().is_some_and(|node| node.is(&instance));
                    let node =
                        if is_active { active.as_mut() } else { candidate.as_mut().filter(|node| node.is(&instance)) };
                    let Some(node) = node else { continue };
                    match event {
                        ServiceEvent::Prepared if !is_active && !node.prepared => {
                            node.prepared = true;
                        }
                        ServiceEvent::Serving
                            if is_active && node.prepared && (!node.serving || node.cancel_requested) =>
                        {
                            node.serving = true;
                            if node.cancel_requested {
                                node.cancel_requested = false;
                                node.yielding = false;
                            }
                            line(&mut log, &format!("node {} is serving as {:?}", node.child.pid, node.child.identity));
                        }
                        ServiceEvent::Yielded if is_active && !node.yielded => {
                            node.yielded = true;
                        }
                        _ => {
                            line(&mut log, "node reported an invalid lifecycle transition");
                            node.child.terminate()?;
                        }
                    }
                }
            }
        }
    })();
    drop(events);
    // The jobs allow only explicit breakaway, so detached world work is not
    // terminated here. Release the host's profile after both nodes are gone.
    for node in [candidate.take(), active.take()].into_iter().flatten() {
        if let Err(error) = node.child.terminate() {
            line(&mut log, &format!("ending foreground process: {error:#}"));
        }
    }
    drop(profile_lease);
    result
}

fn reconcile(
    config: &Config,
    active: &mut Option<Node>,
    candidate: &mut Option<Node>,
    sender: &SyncSender<Event>,
    log: &mut RollingLog,
) -> anyhow::Result<()> {
    let full = match &config.target {
        Some(target) => account::interactive(target, active.as_ref().map(|node| node.child.identity))
            .context("finding the user's interactive logon")?,
        None => None,
    };
    let desired = if config.kind == Kind::Hub || config.systems() {
        Identity::System
    } else {
        full.as_ref().map(|(identity, _)| *identity).unwrap_or(Identity::S4u)
    };
    if active.as_ref().is_some_and(|node| node.child.identity == desired) {
        if let Some(node) = candidate.take() {
            node.child.terminate()?;
        }
        cancel_yield(active, log);
        return Ok(());
    }
    if candidate.as_ref().is_some_and(|node| node.child.identity != desired) {
        candidate.take().expect("matched candidate").child.terminate()?;
        cancel_yield(active, log);
    }
    if candidate.is_none() {
        let login = match &config.target {
            Some(target) => {
                Some(account::login(target, full).context("preparing the user's primary token and profile")?)
            }
            None => None,
        };
        let unadmitted = login.as_ref().and_then(|login| login.unadmitted.clone());
        let child = Child::spawn(config, login, sender.clone()).context("launching the user's foreground process")?;
        line(log, &format!("prepared process {} as {:?}", child.pid, child.identity));
        if let Some(why) = unadmitted {
            line(log, &format!("the user's desktop programs may not look at process {}: {why}", child.pid));
        }
        let node = Node {
            child,
            prepared: config.kind == Kind::Hub,
            serving: config.kind == Kind::Hub,
            yielding: false,
            cancel_requested: false,
            yielded: false,
        };
        if config.kind == Kind::Hub {
            *active = Some(node);
        } else {
            *candidate = Some(node);
        }
    }
    Ok(())
}

fn advance(active: &mut Option<Node>, candidate: &mut Option<Node>, log: &mut RollingLog) -> anyhow::Result<()> {
    if !candidate.as_ref().is_some_and(|node| node.prepared) {
        return Ok(());
    }
    match active {
        None => {
            // Once activated, this is the active node even before Serving is
            // received; a later identity change must drain it, never kill it
            // as if it were still an unused standby.
            let mut node = candidate.take().expect("prepared candidate");
            node.child.send(ServiceCommand::Activate)?;
            *active = Some(node);
        }
        Some(node) if node.serving && !node.yielding => {
            node.child.send(ServiceCommand::YieldWhenIdle)?;
            node.yielding = true;
            line(log, "successor ready; requesting an idle handoff");
        }
        Some(_) => {}
    }
    Ok(())
}

fn cancel_yield(active: &mut Option<Node>, log: &mut RollingLog) {
    if let Some(node) = active.as_mut().filter(|node| node.yielding && !node.yielded && !node.cancel_requested) {
        match node.child.send(ServiceCommand::CancelYield) {
            Ok(()) => node.cancel_requested = true,
            Err(error) => line(log, &format!("canceling handoff: {error:#}")),
        }
    }
}

fn line(file: &mut RollingLog, text: &str) {
    // Diagnostic IO must not take a working node offline. A failed log write
    // is surfaced to the Windows debugger without restarting the process.
    if let Err(error) = file.write(format!("{text}\n").as_bytes()) {
        let message: Vec<u16> = format!("fastctx guardian log failed: {error}\n").encode_utf16().chain([0]).collect();
        unsafe {
            windows_sys::Win32::System::Diagnostics::Debug::OutputDebugStringW(message.as_ptr());
        }
    }
}
