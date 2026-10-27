//! Local multiplexing retains the ordinary watch admission and routing for each
//! subscription. Per-subscription workers and flow windows isolate slow readers.

use crate::Node;
use fastctx_net::{
    watch_flow::Flow,
    watch_ipc::{self, Output},
};
use fastctx_platform::ipc::{FrameReader, FrameWriter};
use fastctx_types::{
    ipc::{Failure, Peer},
    watching::local::{Command, Packet, Subscription},
};
use std::{
    collections::HashMap,
    io,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};
use tokio::sync::{mpsc, watch};

struct Worker {
    request: Subscription,
    next: Arc<AtomicU64>,
    consumed: watch::Sender<Option<u64>>,
    task: tokio::task::JoinHandle<()>,
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
struct Task(tokio::task::JoinHandle<io::Result<()>>);
impl Drop for Task {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub(super) async fn serve(node: Arc<Node>, peer: Peer, mut reader: FrameReader, writer: FrameWriter) -> io::Result<()> {
    let (commands, mut incoming) = mpsc::channel(1);
    let _reader = Task(tokio::spawn(async move {
        loop {
            let command = watch_ipc::command(&mut reader).await;
            let end = !matches!(command, Ok(Some(_)));
            if commands.send(command).await.is_err() || end {
                return Ok(());
            }
        }
    }));
    let (output, queue) = mpsc::channel(watch_ipc::SUBSCRIPTIONS + 1);
    let mut writer = Task(tokio::spawn(watch_ipc::write(writer, queue)));
    let (done, mut ended) = mpsc::channel(watch_ipc::SUBSCRIPTIONS);
    let context = Context { node, peer, output, done };
    let mut workers = Workers::default();
    let mut latest = 0;
    let mut retiring = tokio::task::JoinSet::new();
    let mut views = context.node.views.subscribe();
    loop {
        tokio::select! {
            command = incoming.recv() => match command.unwrap_or(Ok(None))? {
                Some(watch_ipc::ReceivedCommand { command: Command::Subscribe { id, subscription }, memory }) => {
                    if id <= latest { return Err(io::Error::other("local watch subscription numbers must increase")); }
                    latest = id;
                    if workers.0.len() >= watch_ipc::SUBSCRIPTIONS || retiring.len() >= watch_ipc::SUBSCRIPTIONS {
                        return Err(io::Error::other("the local shared watch has reached its subscription limit"));
                    }
                    workers.0.insert(id, context.start(id, *subscription, memory));
                }
                Some(watch_ipc::ReceivedCommand { command: Command::Unsubscribe { id }, .. }) => {
                    if let Some(worker) = workers.0.remove(&id) {
                        worker.task.abort();
                        let output = context.output.clone();
                        retiring.spawn(async move { watch_ipc::cancel(&output, id).await });
                    }
                }
                Some(watch_ipc::ReceivedCommand { command: Command::Consumed { id, number }, .. }) => {
                    if let Some(worker) = workers.0.get(&id) {
                        if number >= worker.next.load(Ordering::Relaxed) {
                            return Err(io::Error::other("the local watch acknowledged a packet not yet sent"));
                        }
                        worker.consumed.send_if_modified(|through| {
                            if through.is_none_or(|through| through < number) { *through = Some(number); true } else { false }
                        });
                    }
                }
                None => return Ok(()),
            },
            Some(id) = ended.recv() => { workers.0.remove(&id); }
            result = &mut writer.0 => return result.map_err(io::Error::other)?,
            Some(result) = retiring.join_next(), if !retiring.is_empty() => { result.map_err(io::Error::other)??; }
            changed = views.changed() => {
                if changed.is_err() { return Ok(()); }
                let refused: Vec<_> = workers.0.iter().filter_map(|(id, worker)| {
                    let result = super::users_voice(&context.node, &context.peer)
                        .and_then(|()| crate::admission::local::admit(&context.node, &context.peer, &worker.request.call()));
                    result.err().filter(crate::obligations::permanent).map(|error| (*id, error))
                }).collect();
                for (id, error) in refused {
                    let worker = workers.0.remove(&id).expect("refused subscription");
                    worker.task.abort();
                    let output = context.output.clone();
                    retiring.spawn(async move {
                        watch_ipc::cancel(&output, id).await?;
                        watch_ipc::send(&output, Packet { subscription: id, number: worker.next.fetch_add(1, Ordering::Relaxed), result: Err(error) }).await
                    });
                }
            }
        }
    }
}

struct Context {
    node: Arc<Node>,
    peer: Peer,
    output: mpsc::Sender<Output>,
    done: mpsc::Sender<u64>,
}
impl Context {
    fn start(&self, id: u64, mut request: Subscription, memory: fastctx_net::reassembly::Reservation) -> Worker {
        let held = request.held.take();
        let retained = request.clone();
        request.held = held;
        let next = Arc::new(AtomicU64::new(0));
        let (consumed, acknowledgements) = watch::channel(None);
        let (node, peer, output, done, sequence) =
            (self.node.clone(), self.peer.clone(), self.output.clone(), self.done.clone(), next.clone());
        let task = tokio::spawn(async move {
            let result =
                crate::gate_waits::asked(run(&node, &peer, id, request, memory, &sequence, &output, acknowledgements))
                    .await;
            if result.is_err() && watch_ipc::cancel(&output, id).await.is_err() {
                let _ = done.send(id).await;
                return;
            }
            if let Err(error) = watch_ipc::send(
                &output,
                Packet {
                    subscription: id,
                    number: sequence.fetch_add(1, Ordering::Relaxed),
                    result: result.map(|()| None),
                },
            )
            .await
            {
                node.log.line(format_args!("ending a local shared watch: {error}"));
            }
            let _ = done.send(id).await;
        });
        Worker { request: retained, next, consumed, task }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run(
    node: &Arc<Node>,
    peer: &Peer,
    id: u64,
    mut request: Subscription,
    memory: fastctx_net::reassembly::Reservation,
    next: &AtomicU64,
    output: &mpsc::Sender<Output>,
    mut consumed: watch::Receiver<Option<u64>>,
) -> Result<(), Failure> {
    super::users_voice(node, peer)?;
    let held = request.held.take();
    crate::admission::local::admit(node, peer, &request.call())?;
    let mut watching = tokio::time::timeout(
        watch_ipc::TIMEOUT,
        crate::talk::watch(node, request.resident, request.machine, request.composer, held, request.now, request.goal),
    )
    .await
    .map_err(|_| Failure::new("opening the local watch did not finish within 30 s"))??;
    drop(memory);
    let mut flow = Flow::default();
    loop {
        let piece = watching.recv().await.map_err(|e| Failure::new(e.to_string()))?;
        let Some(piece) = piece else {
            return Ok(());
        };
        let packet =
            Packet { subscription: id, number: next.fetch_add(1, Ordering::Relaxed), result: Ok(Some(piece?)) };
        let bytes = fastctx_net::watch_queue::size(&packet);
        flow.ready(bytes, &mut consumed).await?;
        let number = packet.number;
        watch_ipc::send(output, packet).await.map_err(|e| Failure::new(e.to_string()))?;
        flow.sent(number, bytes);
    }
}
