//! User-authored machine descriptions and controller-private notes.
//!
//! Descriptions enter the encrypted inventory; private notes are overlaid only when a local
//! surface renders a node. Each file belongs to one World and one value, so editing one
//! machine cannot overwrite another machine's note. No private note enters a network frame.

use super::client::NodeView;
use super::{WorldPaths, load_config, validate_node_name};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// Maximum Unicode scalar values in either layer; rejected at every input boundary.
pub(crate) const MAX_NOTE_CHARS: usize = 256;
const FORMAT_VERSION: u32 = 1;
const MAX_FILE_BYTES: u64 = 16 * 1024;

#[derive(Deserialize, Serialize)]
struct NoteFile {
    version: u32,
    world_id: String,
    text: String,
}

/// Checks length without altering whitespace, line breaks, or other user-authored bytes.
pub(crate) fn validate(text: &str) -> Result<(), String> {
    let length = text.chars().count();
    if length > MAX_NOTE_CHARS {
        Err(format!(
            "A machine description or private note may contain at most {MAX_NOTE_CHARS} characters; this value has {length}."
        ))
    } else {
        Ok(())
    }
}

fn description_path(paths: &WorldPaths) -> PathBuf {
    paths.dir.join("description.json")
}

fn private_path(paths: &WorldPaths, name: &str) -> Result<PathBuf, String> {
    validate_node_name(name)?;
    Ok(paths.dir.join("notes").join(format!("{name}.json")))
}

fn world_id(paths: &WorldPaths) -> Result<String, String> {
    load_config(paths)?
        .map(|config| config.world_id)
        .ok_or_else(|| "This machine is not enrolled in a World.".to_string())
}

fn read(path: &Path, world: &str) -> Result<String, String> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(String::new()),
        Err(error) => {
            return Err(format!(
                "Cannot read {}: {error}",
                crate::paths::display_path(path)
            ));
        }
    };
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("Cannot read {}: {error}", crate::paths::display_path(path)))?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(format!(
            "The note file {} is too large.",
            crate::paths::display_path(path)
        ));
    }
    let saved: NoteFile = serde_json::from_slice(&bytes)
        .map_err(|error| format!("Cannot parse {}: {error}", crate::paths::display_path(path)))?;
    if saved.version != FORMAT_VERSION {
        return Err(format!(
            "The note file {} uses unsupported format {}; this build reads format {FORMAT_VERSION}.",
            crate::paths::display_path(path),
            saved.version
        ));
    }
    if saved.world_id != world {
        return Err(format!(
            "The note file {} belongs to a different World.",
            crate::paths::display_path(path)
        ));
    }
    validate(&saved.text)?;
    Ok(saved.text)
}

fn save(path: &Path, world: String, text: &str) -> Result<(), String> {
    validate(text)?;
    // Refuse to overwrite a newer or damaged value as if it had merely been empty.
    read(path, &world)?;
    let parent = path
        .parent()
        .ok_or_else(|| "The note path has no parent.".to_string())?;
    crate::edit::private_storage::ensure_private_directory(parent, "World notes")?;
    let bytes = serde_json::to_vec_pretty(&NoteFile {
        version: FORMAT_VERSION,
        world_id: world,
        text: text.to_string(),
    })
    .map_err(|error| format!("Cannot encode the note: {error}"))?;
    let write = || -> std::io::Result<()> {
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        temporary.write_all(&bytes)?;
        temporary.as_file().sync_all()?;
        temporary.persist(path).map_err(|error| error.error)?;
        Ok(())
    };
    write().map_err(|error| format!("Cannot save {}: {error}", crate::paths::display_path(path)))
}

/// Reads the description owned by this machine, without loading any key material.
pub(crate) fn description(paths: &WorldPaths) -> Result<String, String> {
    read(&description_path(paths), &world_id(paths)?)
}

/// Persists the description; the caller separately requests an encrypted inventory publish.
pub(crate) fn save_description(paths: &WorldPaths, text: &str) -> Result<(), String> {
    let world = world_id(paths)?;
    paths.ensure()?;
    save(&description_path(paths), world, text)
}

/// Saves or clears one private note, with no network activity.
pub(crate) fn save_private_note(paths: &WorldPaths, name: &str, text: &str) -> Result<(), String> {
    let world = world_id(paths)?;
    paths.ensure()?;
    save(&private_path(paths, name)?, world, text)
}

/// Adds private notes to local presentation copies only. Invalid remote descriptions and
/// damaged local notes fail explicitly instead of silently changing the effective label.
pub(crate) fn annotate(paths: &WorldPaths, nodes: &mut [NodeView]) -> Result<(), String> {
    let world = world_id(paths)?;
    for node in nodes {
        if let Some(inventory) = &node.inventory {
            validate(&inventory.description)?;
        }
        let note = read(&private_path(paths, &node.name)?, &world)?;
        node.private_note = (!note.is_empty()).then_some(note);
    }
    Ok(())
}

/// A single quoted data value for a model-visible or terminal-visible row. Escaping the
/// delimiters also prevents a pasted note from resembling a FastCtx structural marker.
pub(crate) fn quoted(text: &str) -> String {
    let mut output = String::with_capacity(text.len() + 2);
    output.push('"');
    for ch in text.chars() {
        match ch {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            ch if ch.is_control()
                || matches!(ch, '=' | '-' | '\u{061c}' | '\u{200b}'..='\u{200f}' | '\u{2028}'..='\u{202e}' | '\u{2060}'..='\u{2069}' | '\u{feff}') =>
            {
                use std::fmt::Write;
                let _ = write!(output, "\\u{:04x}", ch as u32);
            }
            ch => output.push(ch),
        }
    }
    output.push('"');
    output
}
