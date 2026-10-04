//! A machine's own name in the files of a town it carries into a World it
//! joined under another name: the record's machines and places, where
//! journal entries were written and where their attachments are kept. It is
//! the same machine; only what it is called changed. Entry ids keep the name
//! they were written under, as ids never change.

use fastctx_types::MachineName;
use fastctx_types::project::{Description, EntryStamp, Kept, Record, Writer};

/// `bytes`, the file at `path` in a ledger, with `old` called `new`.
pub fn file(path: &str, bytes: &[u8], old: &MachineName, new: &MachineName) -> Result<Vec<u8>, String> {
    let parts: Vec<&str> = path.split('/').collect();
    let rename = |machine: &mut MachineName| {
        if machine == old {
            *machine = new.clone();
        }
    };
    let writer = |writer: &mut Writer| {
        if let Writer::Resident { address } = writer
            && address.machine == *old
        {
            address.machine = new.clone();
        }
    };
    let renamed = match parts.as_slice() {
        ["project", _] => {
            let mut record: Record = parse(path, bytes)?;
            record.machines.iter_mut().for_each(rename);
            record.places.iter_mut().for_each(|place| rename(&mut place.location.machine));
            if let Some(canon) = &mut record.canon {
                rename(&mut canon.machine);
            }
            rename(&mut record.on);
            writer(&mut record.by);
            pretty(&record)
        }
        ["journals", _, "description", _] => {
            let mut description: Description = parse(path, bytes)?;
            rename(&mut description.on);
            writer(&mut description.by);
            pretty(&description)
        }
        ["journals", _, "entries", _, "entry.json"] => {
            let mut stamp: EntryStamp = parse(path, bytes)?;
            rename(&mut stamp.machine);
            writer(&mut stamp.by);
            for attachment in &mut stamp.attachments {
                if let Kept::OnMachine { location } = &mut attachment.kept {
                    rename(&mut location.machine);
                }
            }
            pretty(&stamp)
        }
        _ => return Ok(bytes.to_vec()),
    };
    Ok(renamed)
}

fn parse<T: serde::de::DeserializeOwned>(path: &str, bytes: &[u8]) -> Result<T, String> {
    serde_json::from_slice(bytes).map_err(|error| format!("{path}: {error}"))
}

fn pretty(value: &impl serde::Serialize) -> Vec<u8> {
    let mut bytes = serde_json::to_vec_pretty(value).expect("the World's types always serialize");
    bytes.push(b'\n');
    bytes
}
