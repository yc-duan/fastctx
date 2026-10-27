//! Subscription commands can include a large held-history vector. The command
//! itself is bounded and assembled before it can change any subscription.
use crate::{
    data::{ReadHalf, WriteHalf},
    reassembly::{Buffer, Reservation},
};
use fastctx_types::watching::Command;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct Header {
    watch_command: usize,
}

pub struct ReceivedCommand {
    pub command: Command,
    pub memory: Reservation,
}

pub async fn send(stream: &mut WriteHalf, command: &Command) -> Result<(), String> {
    let size = crate::watch_queue::size(command);
    if size > super::MAX_PACKET {
        return Err("the watch subscription command exceeds 64 MiB".into());
    }
    let _memory = Reservation::new(size)?;
    let bytes = serde_json::to_vec(command).map_err(|e| e.to_string())?;
    stream.say(&Header { watch_command: bytes.len() }).await?;
    stream.send(&bytes).await
}

pub async fn receive(stream: &mut ReadHalf) -> Result<Option<ReceivedCommand>, String> {
    let Some(header) = stream.hear_or_end::<Header>().await? else { return Ok(None) };
    if header.watch_command > super::MAX_PACKET {
        return Err("the watch subscription command exceeds 64 MiB".into());
    }
    let mut bytes = Buffer::new(header.watch_command, header.watch_command)?;
    stream.take(header.watch_command as u64, |part| bytes.extend(part)).await?;
    let command = serde_json::from_slice(&bytes).map_err(|e| format!("the watch subscription command: {e}"))?;
    Ok(Some(ReceivedCommand { command, memory: bytes.into_reservation() }))
}
