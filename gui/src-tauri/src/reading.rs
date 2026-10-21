//! A text file read for the user from the machine that has it: the bytes
//! the core's read hands over, collected and taken as UTF-8 text. A piece
//! cut from the middle of a file may begin or end inside a character; those
//! few bytes at the cut are not counted against it. Anything else that is
//! not UTF-8 is said to be not text, never decoded by guess.

use fastctx_client::file_read::Read;
use fastctx_client::{Reading, World};
use fastctx_types::access::Written;
use fastctx_types::file_read::{FileRange, FileReadInfo, ReadFileRequest};
use serde::Serialize;
use tauri::ipc::Channel;

use crate::held_reads::{ReadProgress, tell};

#[derive(Serialize)]
pub struct TextRead {
    pub info: FileReadInfo,
    /// The bytes as text, when they are UTF-8; none when they are not.
    pub text: Option<String>,
}

/// A file's bytes whole, for what the page shows as it is (an image).
#[derive(Serialize)]
pub struct BytesRead {
    pub info: FileReadInfo,
    pub base64: String,
}

pub async fn read_bytes(
    world: &World,
    machine: Option<Written>,
    path: String,
    progress: Option<Channel<ReadProgress>>,
) -> Result<BytesRead, String> {
    use base64::Engine as _;
    let mut reading = world.read_file(machine, ReadFileRequest { path, range: FileRange::All });
    tell(reading.progress(), progress);
    let (info, bytes) = whole(&mut reading).await?;
    Ok(BytesRead { info, base64: base64::engine::general_purpose::STANDARD.encode(bytes) })
}

pub async fn read_text(
    world: &World,
    machine: Option<Written>,
    path: String,
    range: FileRange,
    progress: Option<Channel<ReadProgress>>,
) -> Result<TextRead, String> {
    let mut reading = world.read_file(machine, ReadFileRequest { path, range });
    tell(reading.progress(), progress);
    let (info, bytes) = whole(&mut reading).await?;
    let starts_inside = info.offset > 0;
    let ends_inside = info.offset + info.length < info.file.size;
    Ok(TextRead { info, text: text_of(&bytes, starts_inside, ends_inside) })
}

/// All a read hands over, as the file it opened last: a read that started
/// over leaves what came before.
async fn whole(reading: &mut Reading) -> Result<(FileReadInfo, Vec<u8>), String> {
    let mut opened = None;
    let mut bytes = Vec::new();
    while let Some(read) = reading.next().await.map_err(|error| error.to_string())? {
        match read {
            Read::Opened(info) => {
                opened = Some(info);
                bytes.clear();
            }
            Read::Bytes(block) => bytes.extend_from_slice(&block),
        }
    }
    Ok((opened.expect("a read that ended well opened"), bytes))
}

/// The bytes as UTF-8 text, allowing a character cut at either end of a
/// piece taken from inside the file.
fn text_of(bytes: &[u8], starts_inside: bool, ends_inside: bool) -> Option<String> {
    // A UTF-8 character is at most four bytes, so a cut leaves at most three
    // of its continuation bytes at the start.
    let skip = if starts_inside { bytes.iter().take(3).take_while(|&&byte| byte & 0xC0 == 0x80).count() } else { 0 };
    let body = &bytes[skip..];
    match std::str::from_utf8(body) {
        Ok(text) => Some(text.to_string()),
        // Incomplete only at the very end: the cut fell inside a character.
        Err(error) if ends_inside && error.error_len().is_none() => {
            String::from_utf8(body[..error.valid_up_to()].to_vec()).ok()
        }
        Err(_) => None,
    }
}
