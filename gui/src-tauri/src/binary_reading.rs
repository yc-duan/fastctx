//! A single IPC response containing a file's metadata and original bytes.
//! The first four bytes are the little-endian length of a UTF-8 JSON
//! FileReadInfo, followed by that JSON and the file bytes. No base64 or
//! second read is needed to keep the metadata and content together.

use fastctx_client::World;
use fastctx_types::MachineName;
use fastctx_types::file_read::{FileRange, ReadFileRequest};

pub async fn read_binary(
    world: &World,
    machine: Option<MachineName>,
    path: String,
) -> Result<tauri::ipc::Response, String> {
    let mut reading = world
        .read_file(machine, ReadFileRequest { path, range: FileRange::All })
        .await
        .map_err(|error| error.to_string())?;
    let info = reading.info();
    let header = serde_json::to_vec(info).map_err(|error| error.to_string())?;
    let header_length = u32::try_from(header.len()).map_err(|error| error.to_string())?;
    let length = usize::try_from(info.length).map_err(|error| error.to_string())?;
    let capacity = length
        .checked_add(header.len())
        .and_then(|length| length.checked_add(4))
        .ok_or("The file cannot fit in this process's address space")?;
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(capacity).map_err(|error| error.to_string())?;
    bytes.extend_from_slice(&header_length.to_le_bytes());
    bytes.extend_from_slice(&header);
    while let Some(block) = reading.next().await.map_err(|error| error.to_string())? {
        bytes.extend_from_slice(&block);
    }
    if bytes.len() != capacity {
        return Err("The file's received length differs from its metadata".into());
    }
    Ok(tauri::ipc::Response::new(bytes))
}
