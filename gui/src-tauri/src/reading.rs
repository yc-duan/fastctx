//! A text file read for the user from the machine that has it: the bytes
//! the core's read hands over, collected and taken as UTF-8 text. A piece
//! cut from the middle of a file may begin or end inside a character; those
//! few bytes at the cut are not counted against it. Anything else that is
//! not UTF-8 is said to be not text, never decoded by guess.

use fastctx_client::World;
use fastctx_types::MachineName;
use fastctx_types::file_read::{FileRange, FileReadInfo, ReadFileRequest};
use serde::Serialize;

#[derive(Serialize)]
pub struct TextRead {
    pub info: FileReadInfo,
    /// The bytes as text, when they are UTF-8; none when they are not.
    pub text: Option<String>,
}

pub async fn read_text(world: &World, machine: Option<MachineName>, path: String, range: FileRange) -> Result<TextRead, String> {
    let mut reading = world.read_file(machine, ReadFileRequest { path, range }).await.map_err(|error| error.to_string())?;
    let mut bytes = Vec::new();
    while let Some(block) = reading.next().await.map_err(|error| error.to_string())? {
        bytes.extend_from_slice(&block);
    }
    let info = reading.info().clone();
    let starts_inside = info.offset > 0;
    let ends_inside = info.offset + info.length < info.file.size;
    Ok(TextRead { info, text: text_of(&bytes, starts_inside, ends_inside) })
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
