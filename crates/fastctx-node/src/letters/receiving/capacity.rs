//! New letters have a fixed part limit. An already acknowledged legacy letter
//! keeps exactly the finite total recorded in its existing tuples, never more.

use super::*;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
struct Capacity {
    parts: usize,
    stored_bytes_per_part: usize,
}

pub(super) fn admit(directory: &Path, total: usize) -> io::Result<Result<(), String>> {
    let path = directory.join("capacity.json");
    let kept = match read_bounded(&path, 1024)? {
        Some(Ok(bytes)) => Some(
            serde_json::from_slice::<Capacity>(&bytes)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, format!("{}: {error}", path.display())))?,
        ),
        Some(Err(reason)) => return Ok(Err(reason)),
        None => None,
    };
    let capacity = match kept {
        Some(kept) => kept,
        None => {
            let mut legacy = None;
            for entry in std::fs::read_dir(directory)? {
                let entry = entry?;
                let name = entry.file_name();
                let Some(name) = name.to_str() else { continue };
                let Ok(part) = name.parse::<usize>() else { continue };
                let bytes = match read_bounded(&entry.path(), STORED_BYTES)? {
                    Some(Ok(bytes)) => bytes,
                    Some(Err(reason)) => return Ok(Err(reason)),
                    None => return Err(io::Error::new(io::ErrorKind::NotFound, "a legacy fragment disappeared")),
                };
                let (count, piece) = serde_json::from_slice::<(usize, String)>(&bytes).map_err(|error| {
                    io::Error::new(io::ErrorKind::InvalidData, format!("{}: {error}", entry.path().display()))
                })?;
                if count == 0
                    || part >= count
                    || name != part.to_string()
                    || piece.is_empty()
                    || piece.len() > PART_BYTES
                {
                    return Ok(Err(format!("invalid legacy fragment declaration: {}", entry.path().display())));
                }
                if legacy.is_some_and(|kept| kept != count) {
                    return Ok(Err("legacy fragments disagree about their total".to_string()));
                }
                legacy = Some(count);
            }
            let parts = match legacy {
                Some(parts) => parts,
                None if total <= MAX_PARTS => total,
                None => {
                    return Ok(Err(format!(
                        "a new letter may have at most {MAX_PARTS} fragments; none were previously acknowledged here"
                    )));
                }
            };
            Capacity { parts, stored_bytes_per_part: STORED_BYTES }
        }
    };
    if total != capacity.parts || capacity.stored_bytes_per_part != STORED_BYTES {
        return Ok(Err(format!(
            "letter fragments exceed or disagree with their recorded {}-part capacity",
            capacity.parts
        )));
    }
    publish(&path, &serde_json::to_vec(&capacity).map_err(io::Error::other)?, Permissions::Private)?;
    Ok(Ok(()))
}
