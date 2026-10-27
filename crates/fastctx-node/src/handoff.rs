//! A node a guardian keeps, and how it hands the World to its successor
//! (`world-network.md`, 加入了就一直连着).
//!
//! On Windows a guardian runs as the system and keeps the user's node
//! running: under the user's restricted identity before they sign in, under
//! their full one after. To change identities it starts a successor, which
//! says it is prepared and waits; it asks the running node to yield when
//! idle; the running node stops starting turns, lets the running ones end,
//! says it yielded and exits; the successor is then told to take over.
//! Words that come meanwhile are kept and go in once the successor serves.
//!
//! Idle here means: no Codex turn runs or awaits its durable outcome, none
//! is being started, and no call is being answered. Claude Code's sessions run
//! in its own processes and go on across the change; a watch is a stream,
//! not a call, and its watcher comes back to the successor.

use std::fs::File;
use std::io::Write as _;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use fastctx_platform::service_channel::{ServiceCommand, ServiceEvent};
use tokio::sync::Notify;

use crate::Node;

pub(crate) const WAITING: &str = "the node is handing over to its successor; the words go in once it serves";

/// What a guardian handed the node: the channel, and which identity it
/// runs under.
pub struct Supervision {
    pub control: File,
    pub events: File,
    pub identity: Identity,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Identity {
    /// The user's restricted identity, before they sign in.
    S4u,
    /// The user's full identity, from their signed-in session.
    Interactive,
    /// SYSTEM, for the managed computer SYSTEM installed for itself.
    System,
}

impl Supervision {
    /// Says the node is prepared, and waits for the guardian to let it take
    /// the World. The pipe reader remains active during lock acquisition
    /// and recovery, so losing the guardian stops even a node not serving.
    pub(crate) fn prepare(mut self) -> std::io::Result<Prepared> {
        ServiceEvent::Prepared.write(&mut self.events)?;
        let (commands, control) = mpsc::channel();
        std::thread::spawn(move || {
            loop {
                match ServiceCommand::read(&mut self.control) {
                    Ok(Some(command)) => {
                        if commands.send(command).is_err() {
                            return;
                        }
                    }
                    Ok(None) => {
                        let _ = writeln!(std::io::stderr(), "the guardian is gone; a node nobody keeps stops");
                        crate::ending::exit(0);
                    }
                    Err(error) => {
                        let _ = writeln!(std::io::stderr(), "reading the guardian's channel: {error}; stopping");
                        crate::ending::exit(1);
                    }
                }
            }
        });
        loop {
            match control.recv().expect("the pipe reader exits the process on closure") {
                ServiceCommand::Activate => {
                    return Ok(Prepared { control, events: self.events, identity: self.identity });
                }
                // A yield asked of a node not yet serving has nothing to
                // yield; the guardian asks only nodes that serve.
                ServiceCommand::YieldWhenIdle | ServiceCommand::CancelYield => {}
            }
        }
    }
}

pub(crate) struct Prepared {
    control: mpsc::Receiver<ServiceCommand>,
    events: File,
    identity: Identity,
}

/// Whether the node may start turns, and what it is in the middle of.
#[derive(Default)]
pub(crate) struct Handoff {
    state: Mutex<State>,
    changed: Notify,
    watching: std::sync::atomic::AtomicBool,
}

#[derive(Default)]
struct State {
    /// The guardian asked for the World to be handed on when idle.
    yielding: bool,
    guardian: bool,
    maintenance: bool,
    /// Idle while yielding: the node is on its way out, and takes nothing
    /// more on.
    committed: bool,
    /// Codex turns being started now.
    starting: usize,
    /// Calls being answered now.
    calls: usize,
}

/// Something under way the node waits for before it yields; dropping it
/// says it is done.
pub(crate) struct Busy {
    handoff: Arc<Handoff>,
    kind: Kind,
    _work: fastctx_platform::doing::Work,
}

enum Kind {
    Starting,
    Call,
}

impl Drop for Busy {
    fn drop(&mut self) {
        {
            let mut state = self.handoff.state.lock().expect("never poisoned");
            match self.kind {
                Kind::Starting => state.starting -= 1,
                Kind::Call => state.calls -= 1,
            }
        }
        self.handoff.changed.notify_waiters();
    }
}

impl Handoff {
    pub(crate) async fn pause_for_uninstall(&self) {
        {
            let mut state = self.state.lock().expect("never poisoned");
            state.maintenance = true;
            state.yielding = true;
        }
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self.state.lock().expect("never poisoned").starting == 0 {
                return;
            }
            changed.await;
        }
    }
    pub fn yielding(&self) -> bool {
        self.state.lock().expect("never poisoned").yielding
    }

    pub fn cancel(&self) -> bool {
        let mut state = self.state.lock().expect("never poisoned");
        if state.committed {
            return false;
        }
        state.maintenance = false;
        state.yielding = state.guardian;
        drop(state);
        self.changed();
        true
    }
    /// May words be submitted now? Even a busy thread may finish before
    /// `turn/start` reaches it, so all new submissions wait while yielding.
    pub fn start_turn(self: &Arc<Self>) -> Option<Busy> {
        let mut state = self.state.lock().expect("never poisoned");
        if state.committed || state.yielding {
            return None;
        }
        state.starting += 1;
        Some(Busy {
            handoff: self.clone(),
            kind: Kind::Starting,
            _work: fastctx_platform::doing::Work::begin("handoff: starting a turn"),
        })
    }

    /// A call is being answered; `None` once the node is on its way out.
    pub fn call(self: &Arc<Self>) -> Option<Busy> {
        let mut state = self.state.lock().expect("never poisoned");
        if state.committed {
            return None;
        }
        state.calls += 1;
        Some(Busy {
            handoff: self.clone(),
            kind: Kind::Call,
            _work: fastctx_platform::doing::Work::begin("handoff: answering a call"),
        })
    }

    /// Something the node waits on to yield may have changed, such as a
    /// turn ending.
    pub fn changed(&self) {
        self.changed.notify_waiters();
    }

    /// Why words wait while the node hands over.
    pub(crate) fn waiting_why(_node: &Node) -> String {
        WAITING.into()
    }
}

/// Follows the guardian's commands until it is gone, and exits once the
/// node has yielded. The guardian's channel closing means it is gone, and a
/// node nobody keeps does not stay.
pub(crate) fn follow(node: Arc<Node>, supervision: Prepared) {
    let Prepared { control, events, identity } = supervision;
    node.log.line(format_args!("serving under the guardian, as {identity:?}"));
    let events = Arc::new(Mutex::new(events));
    let say = {
        let node = node.clone();
        let events = events.clone();
        move |event: ServiceEvent| {
            if let Err(error) = event.write(&mut *events.lock().expect("never poisoned")) {
                node.log.line(format_args!("telling the guardian {event:?}: {error}"));
            }
        }
    };
    say(ServiceEvent::Serving);
    let watching = node.clone();
    let say_yielded = say.clone();
    node.handoff.watching.store(true, std::sync::atomic::Ordering::SeqCst);
    tokio::spawn(async move { yield_when_idle(watching, say_yielded).await });
    let reading = node.clone();
    let runtime = tokio::runtime::Handle::current();
    std::thread::spawn(move || {
        loop {
            match control.recv().expect("the pipe reader exits the process on closure") {
                ServiceCommand::YieldWhenIdle => {
                    reading.log.line("the guardian asks for the World to be handed on when idle");
                    let mut state = reading.handoff.state.lock().expect("never poisoned");
                    state.guardian = true;
                    state.yielding = true;
                    drop(state);
                    reading.handoff.changed();
                }
                ServiceCommand::CancelYield => {
                    let resumed = {
                        let mut state = reading.handoff.state.lock().expect("never poisoned");
                        let resumed = state.guardian && !state.committed;
                        if resumed {
                            state.guardian = false;
                            state.yielding = state.maintenance;
                        }
                        resumed
                    };
                    if resumed {
                        reading.log.line("the guardian called the handover off; serving on");
                        say(ServiceEvent::Serving);
                        reading.handoff.changed();
                        crate::speech::resume(&reading, &runtime);
                    }
                }
                // Activate is for a node not yet serving.
                ServiceCommand::Activate => {}
            }
        }
    });
}

/// A node no guardian keeps hands the World over: a service now keeps
/// this World's node, and the service's node waits for the World. It goes
/// as a kept one does, once idle, with nobody to tell.
pub(crate) fn hand_over(node: Arc<Node>) {
    node.log.line("a service keeps this World's node now: handing the World over once idle");
    {
        let mut state = node.handoff.state.lock().expect("never poisoned");
        state.maintenance = true;
        state.yielding = true;
    }
    node.handoff.changed();
    if !node.handoff.watching.swap(true, std::sync::atomic::Ordering::SeqCst) {
        tokio::spawn(async move { yield_when_idle(node, |_| {}).await });
    }
}

/// Waits until the node yields: asked to, with no Codex turn running or
/// starting, all reported ends settled, and no call being answered. Then
/// nothing more is taken on, the guardian is told, and the node exits; its
/// successor takes the World.
async fn yield_when_idle(node: Arc<Node>, say: impl Fn(ServiceEvent)) {
    loop {
        let changed = node.handoff.changed.notified();
        tokio::pin!(changed);
        changed.as_mut().enable();
        {
            let mut state = node.handoff.state.lock().expect("never poisoned");
            if state.yielding && state.starting == 0 && state.calls == 0 && node.codex.settled() {
                state.committed = true;
                break;
            }
        }
        changed.await;
    }
    node.log.line("idle: handing the World on to the successor");
    say(ServiceEvent::Yielded);
    crate::ending::exit(0);
}
