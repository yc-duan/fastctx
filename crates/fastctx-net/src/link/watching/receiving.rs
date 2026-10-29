use super::{Mode, Opening, Pool, Queued};
use crate::{data::Stream, link::Link, sign::Domain, watch_mux};
use fastctx_types::{
    ipc::Failure,
    session::Watched,
    watching::{Command, Event, Packet},
};
use std::sync::Arc;
use tokio::sync::mpsc;

struct Writer(tokio::task::JoinHandle<Result<(), String>>);
impl Drop for Writer {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub(super) async fn run(
    pool: Arc<Pool>,
    link: Arc<Link>,
    mut commands: mpsc::Receiver<Queued>,
    stream: Stream,
) -> Result<(), String> {
    let (mut reader, mut write) = stream.split();
    let Some(first) = reader.hear_or_end::<serde_json::Value>().await? else { return Ok(()) };
    if first.get("watch_mux").is_none() {
        pool.legacy();
        let mut first = Some(first);
        let mut next = 0;
        while let Some(signed) = crate::watch_stream::receive_half(&mut reader, first.take()).await? {
            #[derive(serde::Deserialize)]
            struct Legacy {
                watch: u64,
                n: u64,
                watched: Watched,
            }
            let piece: Legacy = link.unseal_packed(&pool.name, Domain::Event, &signed).await?;
            if piece.watch != pool.watch || piece.n != next {
                return Err("the legacy watch sent a piece out of order".into());
            }
            next += 1;
            let sender = {
                let mut state = pool.state.lock().expect("never poisoned");
                state.slots.get_mut(&1).map(|slot| {
                    slot.memory = None;
                    if let Some(opened) = slot.opened.take() {
                        let _ = opened.send(Ok(()));
                    }
                    slot.pieces.clone()
                })
            };
            let Some(sender) = sender else { return Ok(()) };
            let history = !matches!(piece.watched, Watched::Live { .. } | Watched::Now { .. });
            if history {
                for watched in crate::history::split(piece.watched) {
                    if sender.send_wait(Ok((piece.n, watched)), std::time::Duration::from_secs(30)).await.is_err() {
                        return Ok(());
                    }
                }
            } else if sender.send(Ok((piece.n, piece.watched))).is_err() {
                return Ok(());
            }
        }
        return Ok(());
    }
    let acknowledgements = pool.clone();
    let mut writing = Writer(tokio::spawn(async move {
        loop {
            tokio::select! {
                command = commands.recv() => match command {
                    Some(queued) => watch_mux::commands::send(&mut write, &queued.command).await?,
                    None => return Ok(()),
                },
                () = acknowledgements.acknowledged.notified() => {
                    let ready = std::mem::take(&mut *acknowledgements.acknowledgements.lock().expect("never poisoned"));
                    for (id, number) in ready { watch_mux::commands::send(&mut write, &Command::Consumed { id, number }).await?; }
                }
            }
        }
    }));
    let mut input = watch_mux::Reader::default();
    let mut first = Some(first);
    loop {
        let received = tokio::select! {
            received = input.next(&mut reader, first.take()) => received?,
            result = &mut writing.0 => return match result {
                Ok(Ok(())) => Ok(()), Ok(Err(error)) => Err(error), Err(error) => Err(error.to_string()),
            },
        };
        let Some(received) = received else { return Ok(()) };
        let packet: Packet = link.unseal_packed(&pool.name, Domain::Event, &received).await?;
        if packet.watch != pool.watch
            || packet.subscription != received.subscription
            || packet.number != received.number
        {
            return Err("the signed watch packet does not match its stream or subscription".into());
        }
        pool.dispatch(packet)?;
    }
}

impl Pool {
    fn legacy(&self) {
        let others = {
            let mut state = self.state.lock().expect("never poisoned");
            if state.mode == Mode::Closed {
                return;
            }
            state.mode = Mode::Legacy;
            let others = state.slots.split_off(&2);
            if let Some(first) = state.slots.get_mut(&1) {
                first.pending = None;
            }
            others
        };
        for (_, mut slot) in others {
            if let Some(opened) = slot.opened.take() {
                let _ = opened.send(Err(Opening::Separate));
            }
        }
        if self.state.lock().expect("never poisoned").slots.is_empty() {
            self.finish(None);
        }
    }

    fn dispatch(&self, packet: Packet) -> Result<(), String> {
        if let Event::Ready { version, subscriptions } = packet.event {
            if version != watch_mux::VERSION
                || subscriptions != watch_mux::SUBSCRIPTIONS
                || packet.subscription != 0
                || packet.number != 0
            {
                return Err("the peer advertised an unsupported shared watch protocol".into());
            }
            let mut state = self.state.lock().expect("never poisoned");
            if state.mode == Mode::Closed {
                return Ok(());
            }
            if state.mode != Mode::Opening {
                return Err("the shared watch advertised readiness twice".into());
            }
            state.mode = Mode::Multiplexed;
            let mut orphan = !state.slots.contains_key(&1);
            for (id, slot) in &mut state.slots {
                if let Some(request) = slot.pending.take()
                    && *id != 1
                {
                    self.commands
                        .try_send(Queued {
                            command: Command::Subscribe { id: *id, subscription: Box::new(request) },
                            _memory: slot.memory.as_ref().expect("pending request reservation").clone(),
                        })
                        .map_err(|_| "the shared watch command queue is full")?;
                    // Keep one live subscription before retiring the bootstrap;
                    // never transiently exceed the peer's active subscription cap.
                    if orphan {
                        let memory = Arc::new(crate::reassembly::Reservation::new(256)?);
                        self.commands
                            .try_send(Queued { command: Command::Unsubscribe { id: 1 }, _memory: memory })
                            .map_err(|_| "the shared watch command queue is full")?;
                        orphan = false;
                    }
                }
            }
            return Ok(());
        }
        let id = packet.subscription;
        let mut consume = false;
        let mut remove = false;
        {
            let mut state = self.state.lock().expect("never poisoned");
            let Some(slot) = state.slots.get_mut(&id) else {
                return Ok(());
            };
            let failure_ends_gap =
                matches!(packet.event, Event::End { failure: Some(_) }) && packet.number >= slot.next;
            if packet.number != slot.next && !failure_ends_gap {
                return Err("the shared watch sent a subscription packet out of order".into());
            }
            slot.next = packet.number.checked_add(1).ok_or("watch packet numbers exhausted")?;
            match packet.event {
                Event::Opened => {
                    slot.memory = None;
                    if let Some(opened) = slot.opened.take() {
                        let _ = opened.send(Ok(()));
                    }
                    consume = true;
                }
                Event::Piece { watched } => {
                    if slot.opened.is_some() {
                        return Err("the shared watch sent data before admitting its subscription".into());
                    }
                    if slot.pieces.send(Ok((packet.number, watched))).is_err() {
                        remove = true;
                    }
                }
                Event::End { failure } => {
                    let mut slot = state.slots.remove(&id).expect("ended subscription");
                    if let Some(opened) = slot.opened.take() {
                        let _ = opened.send(Err(Opening::Failed(
                            failure.clone().unwrap_or_else(|| Failure::new("the watch ended before opening")),
                        )));
                    }
                    if let Some(failure) = failure {
                        let _ = slot.pieces.send(Err(failure));
                    }
                }
                Event::Ready { .. } => unreachable!("handled Ready"),
            }
        }
        if consume {
            self.command(Command::Consumed { id, number: packet.number }).map_err(|e| e.message)?;
        }
        if remove {
            self.remove(id);
        }
        Ok(())
    }
}
