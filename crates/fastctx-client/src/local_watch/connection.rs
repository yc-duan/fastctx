use super::Shared;
use fastctx_net::watch_ipc::{self, PendingCommand, Received};
use fastctx_platform::ipc::{Connection, FrameWriter};
use fastctx_types::{ipc::Failure, watching::local::Command};
use std::{io, sync::Arc};
use tokio::sync::mpsc;

struct Task(tokio::task::JoinHandle<io::Result<()>>);
impl Drop for Task {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub(super) async fn run(shared: Arc<Shared>, connection: Connection, incoming: mpsc::Receiver<PendingCommand>) {
    let (mut reader, writer) = connection.split();
    let mut writing = Task(tokio::spawn(write(shared.clone(), writer, incoming)));
    let mut pieces = watch_ipc::Reader::default();
    let result = loop {
        tokio::select! {
            result = &mut writing.0 => break match result { Ok(result) => result, Err(error) => Err(io::Error::other(error)) },
            result = pieces.next(&mut reader) => match result {
                Ok(Some(piece)) => if let Err(error) = receive(&shared, piece) { break Err(error); },
                Ok(None) => break Err(io::ErrorKind::UnexpectedEof.into()),
                Err(error) => break Err(error),
            },
        }
    };
    let mut state = shared.state.lock().expect("local watch state");
    let failure = match result {
        Ok(()) => Failure::new("the local shared watch closed"),
        Err(error) => Failure::new(format!("the local shared watch ended: {error}")),
    };
    Shared::fail(&mut state, failure);
}

async fn write(
    shared: Arc<Shared>,
    mut writer: FrameWriter,
    mut incoming: mpsc::Receiver<PendingCommand>,
) -> io::Result<()> {
    loop {
        if shared.closed() {
            return Ok(());
        }
        tokio::select! {
            command = incoming.recv() => {
                let Some(command) = command else { return Ok(()); };
                command.send(&mut writer).await?;
            }
            () = shared.wake.notified() => {}
        }
        let acknowledgements = {
            let mut state = shared.state.lock().expect("local watch state");
            if state.closed {
                return Ok(());
            }
            std::mem::take(&mut state.acknowledged)
        };
        for (id, number) in acknowledgements {
            PendingCommand::new(&Command::Consumed { id, number })?.send(&mut writer).await?;
        }
    }
}

fn receive(shared: &Shared, received: Received) -> io::Result<()> {
    let Received { packet, memory } = received;
    let mut state = shared.state.lock().expect("local watch state");
    let Some(slot) = state.slots.get_mut(&packet.subscription) else {
        return Ok(());
    };
    if packet.number != slot.expected && !(packet.number > slot.expected && packet.result.is_err()) {
        return Err(io::Error::other("the local watch packet is out of order"));
    }
    slot.expected =
        packet.number.checked_add(1).ok_or_else(|| io::Error::other("the local watch exhausted its packet numbers"))?;
    let close = match packet.result {
        Ok(Some(watched)) => slot.sender.send(Ok((packet.number, watched))).is_err(),
        Ok(None) => true,
        Err(failure) => {
            let _ = slot.sender.send(Err(failure));
            true
        }
    };
    drop(memory);
    if close {
        state.slots.remove(&packet.subscription);
        state.acknowledged.remove(&packet.subscription);
        if state.slots.is_empty() {
            state.closed = true;
        } else if PendingCommand::new(&Command::Unsubscribe { id: packet.subscription })
            .ok()
            .is_none_or(|command| shared.commands.try_send(command).is_err())
        {
            Shared::fail(&mut state, Failure::new("the local watch could not unsubscribe its completed viewer"));
        }
        shared.wake.notify_one();
    }
    Ok(())
}
