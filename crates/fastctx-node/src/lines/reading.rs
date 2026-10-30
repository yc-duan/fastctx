//! Bounded row producers for local IPC and remote data connections.

use super::{Cancel, Node, failed, observe, refresh};
use fastctx_net::lines_read::{Content, Piece, Received, Sender};
use fastctx_platform::ipc::{FrameReader, FrameWriter};
use fastctx_types::MachineName;
use fastctx_types::access::Written;
use fastctx_types::access::{Acting, Purpose};
use fastctx_types::ipc::{Failure, NodeFrame};
use fastctx_types::lines::{Part, Reply, Result as Answer};
use fastctx_types::remote::RemoteReply;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use tokio::sync::mpsc;

async fn here(
    node: &Arc<Node>,
    from: MachineName,
    source: String,
    start: u64,
    count: u64,
    sender: &Sender,
    only_notes: bool,
) -> Result<Reply, Failure> {
    let key = (from, source);
    let source = node
        .line_sources
        .0
        .lock()
        .expect("line sources")
        .get(&key)
        .cloned()
        .ok_or_else(|| failed("line source is closed or belongs to another caller"))?;
    refresh(source.clone(), None).await?;
    let cancelled = Cancel::default();
    let cancel = cancelled.0.clone();
    {
        let mut requests = source.requests.lock().expect("line requests");
        requests.retain(|request| request.strong_count() > 0);
        requests.push(Arc::downgrade(&cancel));
        if source.cancelled.load(Ordering::Relaxed) {
            return Err(failed("line source closed"));
        }
    }
    let sender = sender.clone();
    let read_count = tokio::task::spawn_blocking(move || {
        let mut state = source.state.lock().expect("line source");
        let super::State { index, projection, .. } = &mut *state;
        let index = index.as_mut().ok_or_else(|| "line source closed".to_string())?;
        let count = if let Some(projection) = projection {
            index.visit_projected(start, count, &cancel, projection, |line, text, ended, note| {
                sender
                    .blocking_send(Ok(Received::Text(part(line, text, ended, note, only_notes))))
                    .map_err(|error| error.to_string())
            })?
        } else {
            index.visit(start, count, &cancel, |line, text, ended| {
                sender
                    .blocking_send(Ok(Received::Text(part(line, text, ended, None, only_notes))))
                    .map_err(|error| error.to_string())
            })?
        };
        let snapshot = observe(&source.path).map_err(|error| error.to_string())?;
        if (source.job_record.is_none() || source.immutable) && snapshot != state.snapshot
            || !snapshot.same_file(&state.snapshot)
            || snapshot.size < state.snapshot.size
        {
            return Err("the text source changed during reading".to_string());
        }
        Ok::<_, String>(count)
    })
    .await
    .map_err(failed)?
    .map_err(failed)?;
    Ok(Reply { machine: node.machine(), result: Answer::ReadComplete { start, count: read_count } })
}

fn start(
    node: Arc<Node>,
    from: MachineName,
    machine: Option<Written>,
    source: String,
    start: u64,
    count: u64,
    only_notes: bool,
) -> mpsc::Receiver<Result<Received, Failure>> {
    let (sender, receiver) = mpsc::channel(1);
    tokio::spawn(async move {
        tokio::select! {
            () = sender.closed() => {},
            result = async {
                if let Some(machine) = machine.filter(|machine| !node.is_here(machine)) {
                    let (link, account) = node.speaking(&Acting::Here, &machine, Purpose::Hands).await.map_err(failed)?;
                    fastctx_net::lines_read::read(&link, machine.name, account, fastctx_types::lines::ReadRequest { source, start, count }, &sender, only_notes).await
                } else {
                    here(&node, from, source, start, count, &sender, only_notes).await
                }
            } => { let _ = sender.send(result.map(Received::Finished)).await; }
        }
    });
    receiver
}

pub(crate) async fn local(
    node: &Arc<Node>,
    machine: Option<Written>,
    request: fastctx_types::lines::ReadRequest,
    only_notes: bool,
    id: u64,
    reader: &mut FrameReader,
    writer: &mut FrameWriter,
) -> std::io::Result<()> {
    let fastctx_types::lines::ReadRequest { source, start: start_line, count } = request;
    let mut parts = start(node.clone(), node.machine(), machine, source, start_line, count, only_notes);
    let reply = loop {
        let part = tokio::select! { part = parts.recv() => part, _ = reader.recv() => return Ok(()) };
        match part {
            Some(Ok(Received::Text(part))) => {
                writer.send(&serde_json::to_vec(&NodeFrame::LinesRead { id, part }).expect("line piece")).await?
            }
            Some(Ok(Received::Finished(reply))) => break Ok(fastctx_types::Reply::Lines(reply)),
            Some(Err(error)) => break Err(error),
            None => break Err(failed("the line reader stopped without a result")),
        }
    };
    writer.send(&serde_json::to_vec(&NodeFrame::Reply { id, reply }).expect("line reply")).await?;
    writer.drain().await
}

pub(crate) async fn remote(
    node: &Arc<Node>,
    link: &Arc<fastctx_net::link::Link>,
    from: MachineName,
    request: fastctx_types::lines::ReadRequest,
    only_notes: bool,
    read: u64,
    mut stream: fastctx_net::data::Stream,
) -> Result<RemoteReply, Failure> {
    let fastctx_types::lines::ReadRequest { source, start: start_line, count } = request;
    let mut parts = start(node.clone(), from, None, source.clone(), start_line, count, only_notes);
    let mut n = 0;
    loop {
        let part = tokio::select! { part = parts.recv() => part, () = stream.closed() => return Err(failed("line reader disconnected")) };
        let (content, terminal) = match part {
            Some(Ok(Received::Text(part))) => (Content::Text { part }, None),
            Some(Ok(Received::Finished(reply))) => {
                let Answer::ReadComplete { start, count } = reply.result else { unreachable!("line completion") };
                (Content::Finished { start, count }, Some(reply))
            }
            Some(Err(error)) => return Err(error),
            None => return Err(failed("the line reader stopped without a result")),
        };
        stream
            .say(
                &link.sign(fastctx_net::sign::Domain::Event, &Piece { read, n, source: source.clone(), part: content }),
            )
            .await
            .map_err(failed)?;
        if let Some(reply) = terminal {
            stream.finish().await.map_err(failed)?;
            return Ok(RemoteReply::Lines(reply));
        }
        n += 1;
    }
}

fn part(
    line: u64,
    text: String,
    ended: bool,
    note: Option<fastctx_types::run_facts::OutputNote>,
    only_notes: bool,
) -> Part {
    let utf16_length = only_notes.then(|| text.encode_utf16().count() as u64);
    Part { line, text: if only_notes { String::new() } else { text }, utf16_length, ended, note }
}
