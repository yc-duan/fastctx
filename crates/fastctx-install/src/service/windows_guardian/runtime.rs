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
    pub wake_queued: AtomicBool,
}

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
    let result = (|| {
        loop {
            if requests.stop.load(Ordering::SeqCst) || requests.shutdown.load(Ordering::SeqCst) {
                return Ok(());
            }
            if requests.refresh.swap(false, Ordering::SeqCst) {
                refresh = true;
            }
            if requests.adopt.swap(false, Ordering::SeqCst) {
                match super::adopt::adopt(&config) {
                    Ok(()) => line(&mut log, "took the newer World program its user asked for"),
                    Err(error) => line(&mut log, &format!("did not take the World program: {error:#}")),
                }
            }
            if refresh || active.is_none() && candidate.is_none() {
                refresh = false;
                let reconcile = reconcile(&config, &mut active, &mut candidate, &sender, &mut log);
                if let Err(error) = reconcile {
                    line(&mut log, &format!("cannot prepare a node: {error:#}"));
                    // A failed successor must never take a working node down.
                    // Session changes and the independent periodic check retry
                    // it; without any node, SCM immediately retries the host.
                    if active.is_none() {
                        return Err(error);
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
            match events.recv()? {
                Event::Wake => {
                    requests.wake_queued.store(false, Ordering::SeqCst);
                    refresh = true;
                }
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
        let child = Child::spawn(config, login, sender.clone()).context("launching the user's foreground process")?;
        line(log, &format!("prepared process {} as {:?}", child.pid, child.identity));
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
