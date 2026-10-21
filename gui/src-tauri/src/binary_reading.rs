//! A single IPC response containing a file's metadata and original bytes.
//! The first four bytes are the little-endian length of a UTF-8 JSON
//! FileReadInfo, followed by that JSON and the file bytes. No base64 or
//! second read is needed to keep the metadata and content together.

use fastctx_client::World;
use fastctx_client::file_read::Read;
use fastctx_types::access::Written;
use fastctx_types::file_read::{FileRange, ReadFileRequest};
use tauri::ipc::Channel;

use crate::held_reads::{ReadProgress, tell};

pub async fn read_binary(
    world: &World,
    machine: Option<Written>,
    path: String,
    progress: Option<Channel<ReadProgress>>,
) -> Result<tauri::ipc::Response, String> {
    let mut reading = world.read_file(machine, ReadFileRequest { path, range: FileRange::All });
    tell(reading.progress(), progress);
    let mut bytes = Vec::new();
    let mut capacity = 0;
    while let Some(read) = reading.next().await.map_err(|error| error.to_string())? {
        match read {
            // The header, and room for all the file: again from the start
            // when the read starts over.
            Read::Opened(info) => {
                let header = serde_json::to_vec(&info).map_err(|error| error.to_string())?;
                let header_length = u32::try_from(header.len()).map_err(|error| error.to_string())?;
                let length = usize::try_from(info.length).map_err(|error| error.to_string())?;
                capacity = length
                    .checked_add(header.len())
                    .and_then(|length| length.checked_add(4))
                    .ok_or("The file cannot fit in this process's address space")?;
                bytes = Vec::new();
                bytes.try_reserve_exact(capacity).map_err(|error| error.to_string())?;
                bytes.extend_from_slice(&header_length.to_le_bytes());
                bytes.extend_from_slice(&header);
            }
            Read::Bytes(block) => bytes.extend_from_slice(&block),
        }
    }
    if bytes.len() != capacity {
        return Err("The file's received length differs from its metadata".into());
    }
    Ok(tauri::ipc::Response::new(bytes))
}
