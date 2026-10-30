//! One device/account connection carries independently authorized subscriptions.
//! Each producer has one bounded packet in flight; the writer rotates byte chunks.

mod input;

use super::{Opening, Told, live, opening};
use crate::Node;
use fastctx_net::{
    data::Stream,
    link::{Caller, Link},
    watch_mux::{self, Output},
};
use fastctx_types::{
    ipc::Failure,
    remote::RemoteReply,
    watching::{Command, Event, Packet, Subscription},
};
use std::collections::HashMap;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

struct Worker {
    subscription: Subscription,
    next: Arc<AtomicU64>,
    consumed: tokio::sync::watch::Sender<Option<u64>>,
    task: tokio::task::JoinHandle<()>,
    input: mpsc::Sender<input::Incoming>,
    _alive: tokio::sync::watch::Sender<()>,
}
#[derive(Default)]
struct Workers(HashMap<u64, Worker>);
impl Drop for Workers {
    fn drop(&mut self) {
        for worker in self.0.values() {
            worker.task.abort();
        }
    }
}
struct Writer(tokio::task::JoinHandle<Result<(), String>>);
impl Drop for Writer {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub(crate) async fn serve(
    node: Arc<Node>,
    link: Arc<Link>,
    caller: Caller,
    initial: Subscription,
    watch: u64,
    stream: Stream,
) -> Result<RemoteReply, Failure> {
    let (mut reader, writer) = stream.split();
    let (commands, mut receive) = mpsc::channel(1);
    let _reading = Writer(tokio::spawn(async move {
        loop {
            let command = watch_mux::commands::receive(&mut reader).await;
            let end = !matches!(command, Ok(Some(_)));
            if commands.send(command).await.is_err() || end {
                return Ok(());
            }
        }
    }));
    let (output, queue) = mpsc::channel(watch_mux::SUBSCRIPTIONS + 1);
    let mut writer = Writer(tokio::spawn(watch_mux::write(writer, queue)));
    send(
        &link,
        &output,
        Packet {
            watch,
            subscription: 0,
            number: 0,
            event: Event::Ready { version: watch_mux::VERSION, subscriptions: watch_mux::SUBSCRIPTIONS },
        },
    )
    .await?;
    let (done, mut ended) = mpsc::channel(watch_mux::SUBSCRIPTIONS);
    let mut workers = Workers::default();
    let context = Context { node, link, caller, watch, output, done };
    workers.0.insert(1, context.start(1, initial, None));
    let mut latest = 1;
    let mut views = context.node.views.subscribe();
    let mut retiring = tokio::task::JoinSet::new();
    loop {
        if workers.0.is_empty() && retiring.is_empty() {
            return Ok(RemoteReply::Watched);
        }
        tokio::select! {
            command = receive.recv() => match command.unwrap_or(Ok(None)).map_err(Failure::new)? {
                Some(watch_mux::commands::ReceivedCommand { command: Command::Subscribe { id, subscription }, memory }) => {
                    if id <= latest { return Err(Failure::new("watch subscription numbers must increase")); }
                    latest = id;
                    if workers.0.len() >= watch_mux::SUBSCRIPTIONS || retiring.len() >= watch_mux::SUBSCRIPTIONS {
                        return Err(Failure::new("the shared watch has reached its subscription limit"));
                    }
                    workers.0.insert(id, context.start(id, *subscription, Some(memory)));
                }
                Some(watch_mux::commands::ReceivedCommand { command: Command::Unsubscribe { id }, .. }) => {
                    if let Some(worker) = workers.0.remove(&id) {
                        worker.task.abort();
                        let output = context.output.clone();
                        retiring.spawn(async move { cancel(&output, id).await });
                    }
                    if workers.0.is_empty() && retiring.is_empty() { return Ok(RemoteReply::Watched); }
                }
                Some(watch_mux::commands::ReceivedCommand { command: Command::Consumed { id, number }, .. }) => {
                    if let Some(worker) = workers.0.get(&id) {
                        if number >= worker.next.load(Ordering::Relaxed) { return Err(Failure::new("the watch acknowledged a packet not yet sent")); }
                        worker.consumed.send_if_modified(|through| {
                            if through.is_none_or(|through| through < number) { *through = Some(number); true } else { false }
                        });
                    }
                }
                Some(watch_mux::commands::ReceivedCommand { command: Command::Composer { id, input }, memory }) => {
                    if let Some(worker) = workers.0.get(&id) {
                        let incoming = input::Incoming { input: *input, memory, received: tokio::time::Instant::now() };
                        if worker.subscription.documents.is_none() || worker.input.try_send(incoming).is_err() {
                            let worker = workers.0.remove(&id).expect("the document subscription exists");
                            worker.task.abort();
                            let (link, output, next) = (context.link.clone(), context.output.clone(), worker.next);
                            retiring.spawn(async move {
                                cancel(&output, id).await?;
                                send(&link, &output, Packet { watch, subscription: id, number: next.fetch_add(1, Ordering::Relaxed),
                                    event: Event::End { failure: Some(Failure::new("the document input queue is full or unavailable")) } }).await
                            });
                        }
                    }
                }
                None => return Ok(RemoteReply::Watched),
            },
            Some(id) = ended.recv() => {
                workers.0.remove(&id);
                if workers.0.is_empty() && retiring.is_empty() { return Ok(RemoteReply::Watched); }
            }
            result = &mut writer.0 => {
                return Err(Failure::new(match result { Ok(Ok(())) => "the shared watch writer ended".into(), Ok(Err(e)) => e, Err(e) => e.to_string() }));
            }
            Some(result) = retiring.join_next(), if !retiring.is_empty() => { result.map_err(|e| Failure::new(e.to_string()))??; }
            changed = views.changed() => {
                if changed.is_err() { return Ok(RemoteReply::Watched); }
                let mut refused = Vec::new();
                for (id, worker) in &workers.0 {
                    if let Err(failure) = crate::admission::admit(&context.node, &context.link, &context.caller, &worker.subscription.call(watch, false))
                        && crate::obligations::permanent(&failure) {
                        refused.push((*id, failure));
                    }
                }
                for (id, failure) in refused {
                    let worker = workers.0.remove(&id).expect("refused subscription");
                    worker.task.abort();
                    let (link, output, next) = (context.link.clone(), context.output.clone(), worker.next);
                    retiring.spawn(async move {
                        cancel(&output, id).await?;
                        send(&link, &output, Packet { watch, subscription: id, number: next.fetch_add(1, Ordering::Relaxed), event: Event::End { failure: Some(failure) } }).await
                    });
                }
            }
        }
    }
}

struct Context {
    node: Arc<Node>,
    link: Arc<Link>,
    caller: Caller,
    watch: u64,
    output: mpsc::Sender<Output>,
    done: mpsc::Sender<u64>,
}
impl Context {
    fn start(
        &self,
        id: u64,
        mut subscription: Subscription,
        memory: Option<fastctx_net::reassembly::Reservation>,
    ) -> Worker {
        let next = Arc::new(AtomicU64::new(0));
        let (node, link, caller, watch, output, done) = (
            self.node.clone(),
            self.link.clone(),
            self.caller.clone(),
            self.watch,
            self.output.clone(),
            self.done.clone(),
        );
        let mut request = subscription.clone();
        request.held = subscription.held.take();
        let sequence = next.clone();
        let (consumed, mut acknowledgements) = tokio::sync::watch::channel(None);
        let (input, incoming) = mpsc::channel(1);
        let (alive, closed) = tokio::sync::watch::channel(());
        let task = tokio::spawn(async move {
            let result = crate::gate_waits::asked(run(
                &node,
                &link,
                &caller,
                watch,
                id,
                request,
                memory,
                &sequence,
                &output,
                &mut acknowledgements,
                incoming,
                closed,
            ))
            .await;
            let ending = send(
                &link,
                &output,
                Packet {
                    watch,
                    subscription: id,
                    number: sequence.fetch_add(1, Ordering::Relaxed),
                    event: Event::End { failure: result.err() },
                },
            )
            .await;
            if let Err(error) = ending {
                node.log.line(format_args!("ending a shared watch subscription: {}", error.message));
            }
            let _ = done.send(id).await;
        });
        Worker { subscription, next, consumed, task, input, _alive: alive }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run(
    node: &Arc<Node>,
    link: &Arc<Link>,
    caller: &Caller,
    watch: u64,
    id: u64,
    mut request: Subscription,
    memory: Option<fastctx_net::reassembly::Reservation>,
    next: &AtomicU64,
    output: &mpsc::Sender<Output>,
    consumed: &mut tokio::sync::watch::Receiver<Option<u64>>,
    mut incoming: mpsc::Receiver<input::Incoming>,
    closed: tokio::sync::watch::Receiver<()>,
) -> Result<(), Failure> {
    let call = request.call(watch, false);
    let asker = crate::admission::admit(node, link, caller, &call)?;
    let mut admission = fastctx_net::link::admission::Request::call(
        caller.machine.clone(),
        caller.account.clone(),
        tokio::time::Instant::now(),
        &call,
        &node.machine(),
    );
    // Only preparing what it is first told takes a read slot, a shared
    // input's too: the draft's slots are for changes to the draft. A quiet
    // live subscription never occupies execution capacity, and its bounded
    // queue is independent.
    admission.pool = fastctx_types::contract::Pool::Short;
    admission.kind = if request.composer.is_some() { "opening the shared input" } else { "opening a watch" }.into();
    drop(call);
    // Told it is open before the slot is taken, so no slot waits on the line.
    let mut flow = Flow::default();
    flow.deliver(
        link,
        output,
        Packet { watch, subscription: id, number: next.fetch_add(1, Ordering::Relaxed), event: Event::Opened },
        consumed,
    )
    .await?;
    let permit = crate::intake::take(node, link.hub().clone(), admission).await?;
    permit.started();
    if let Some(subscription) = request.documents.take() {
        if request.goal {
            return Err(Failure::not_run("goal observations do not accept document updates"));
        }
        let name = request
            .composer
            .take()
            .ok_or_else(|| Failure::not_run("document synchronization requires a composer watch identity"))?;
        let mut documents = crate::composer::watch_documents(node, &request.resident, name, subscription).await?;
        drop(memory);
        drop(permit);
        let mut updating: Option<tokio::task::JoinHandle<fastctx_types::composer::sync::Frame>> = None;
        loop {
            let frame = tokio::select! {
                frame = documents.next() => match frame? { Some(frame) => frame, None => return Ok(()) },
                Some(input) = incoming.recv(), if updating.is_none() => {
                    updating = Some(tokio::spawn(input::run(node.clone(), link.clone(), caller.clone(), request.resident.clone(), input, closed.clone())));
                    continue;
                }
                result = async { match updating.as_mut() { Some(task) => task.await, None => std::future::pending().await } } => {
                    updating = None;
                    Arc::new(result.map_err(|error| Failure::new(error.to_string()))?)
                }
            };
            flow.deliver(
                link,
                output,
                Packet {
                    watch,
                    subscription: id,
                    number: next.fetch_add(1, Ordering::Relaxed),
                    event: Event::Piece {
                        watched: fastctx_types::session::Watched::Documents {
                            frame: Box::new(Arc::unwrap_or_clone(frame)),
                        },
                    },
                },
                consumed,
            )
            .await?;
        }
    }
    let held = request.held.take();
    let (lease, opened) = if request.goal {
        let account = fastctx_types::access::HubAccount { hub: asker.hub.clone(), account: asker.account.clone() };
        (None, crate::goals::watch::opening(node, &request.resident, &account).await?)
    } else {
        opening(node, &request.resident, request.composer.clone(), held.as_ref()).await?
    };
    drop(held);
    drop(memory);
    drop(permit);
    let _lease = lease;
    let Opening { pieces, mut events, slim } = opened;
    let mut told = Told::new(node, &request.resident, request.now && request.composer.is_none());
    for watched in pieces.into_iter().chain(told.first()) {
        flow.deliver(
            link,
            output,
            Packet {
                watch,
                subscription: id,
                number: next.fetch_add(1, Ordering::Relaxed),
                event: Event::Piece { watched },
            },
            consumed,
        )
        .await?;
    }
    loop {
        let watched = tokio::select! {
            event = events.recv() => match event {
                Ok(Some(event)) => {
                    if request.goal && !crate::goals::watch::is_goal(&event) { continue; }
                    if !crate::goals::visible(node, crate::admission::seen::Looker::Asker(&asker), &event) { continue; }
                    live(Arc::unwrap_or_clone(event), slim.as_ref()).map(|event| fastctx_types::session::Watched::Live { event })
                }
                Ok(None) => return Ok(()),
                Err(error) => return Err(Failure::new(error.to_string())),
            },
            now = told.changed(node) => now,
        };
        if let Some(watched) = watched {
            flow.deliver(
                link,
                output,
                Packet {
                    watch,
                    subscription: id,
                    number: next.fetch_add(1, Ordering::Relaxed),
                    event: Event::Piece { watched },
                },
                consumed,
            )
            .await?;
        }
    }
}

#[derive(Default)]
struct Flow(fastctx_net::watch_flow::Flow);
impl Flow {
    async fn deliver(
        &mut self,
        link: &Arc<Link>,
        output: &mpsc::Sender<Output>,
        packet: Packet,
        consumed: &mut tokio::sync::watch::Receiver<Option<u64>>,
    ) -> Result<(), Failure> {
        let bytes = fastctx_net::watch_queue::size(&packet);
        self.0.ready(bytes, consumed).await?;
        let number = packet.number;
        send(link, output, packet).await?;
        self.0.sent(number, bytes);
        Ok(())
    }
}

async fn send(link: &Arc<Link>, output: &mpsc::Sender<Output>, packet: Packet) -> Result<(), Failure> {
    let encoded = watch_mux::encode(link.clone(), packet).await.map_err(Failure::new)?;
    let (sent, written) = oneshot::channel();
    tokio::time::timeout(Duration::from_secs(30), output.send(Output::Packet { encoded, sent }))
        .await
        .map_err(|_| Failure::new("the shared watch writer queue stayed full for 30 s"))?
        .map_err(|_| Failure::new("the shared watch writer closed"))?;
    written.await.map_err(|_| Failure::new("the shared watch packet was cancelled before it was written"))
}
async fn cancel(output: &mpsc::Sender<Output>, subscription: u64) -> Result<(), Failure> {
    let (cancelled, received) = oneshot::channel();
    tokio::time::timeout(Duration::from_secs(30), async {
        output.send(Output::Cancel { subscription, cancelled }).await.map_err(|_| ())?;
        received.await.map_err(|_| ())
    })
    .await
    .map_err(|_| Failure::new("cancelling the watch packet did not finish within 30 s"))?
    .map_err(|_| Failure::new("the shared watch writer closed"))
}
