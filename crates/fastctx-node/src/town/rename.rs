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
    let writer = |value: &mut Writer| rename_writer(value, old, new);
    let renamed = match parts.as_slice() {
        ["pages", _, "versions", _, "page.json"] => {
            let mut version: fastctx_types::pages::PageVersion = parse(path, bytes)?;
            rename(&mut version.on);
            writer(&mut version.by);
            version.acts.iter_mut().for_each(|act| rename_act(act, old, new));
            pretty(&version)
        }
        ["pages", _, "consent", _] => {
            let mut consent: fastctx_types::pages::PageConsent = parse(path, bytes)?;
            rename(&mut consent.stamp.on);
            writer(&mut consent.stamp.by);
            consent.acts.iter_mut().for_each(|act| rename_act(act, old, new));
            pretty(&consent)
        }
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

/// Rewrites an unpublished memory bundle together, keeping its Markdown stamp consistent.
pub fn memory(files: &mut [fastctx_net::ledger::File], old: &MachineName, new: &MachineName) -> Result<(), String> {
    let rename = |author: &mut fastctx_types::memory::Author| {
        if author.stamp.on == *old {
            author.stamp.on = new.clone();
        }
        rename_writer(&mut author.stamp.by, old, new);
    };
    match fastctx_net::memory::decode(files)? {
        Some(fastctx_net::memory::Mutation::Revision(mut revision)) => {
            rename(&mut revision.author);
            for file in files {
                if file.path == revision.path("json") {
                    file.bytes.0 = serde_json::to_vec(&revision).map_err(|error| error.to_string())?;
                } else if file.path == revision.path("md") {
                    let text = std::str::from_utf8(&file.bytes.0).map_err(|error| error.to_string())?;
                    file.bytes.0 =
                        fastctx_types::memory::markdown::render(&revision.name, revision.kind, text, &revision.author)?
                            .1
                            .into_bytes();
                }
            }
        }
        Some(fastctx_net::memory::Mutation::Evidence(mut evidence)) => {
            rename(&mut evidence.author);
            if let Some(location) = &mut evidence.location
                && location.machine == *old
            {
                location.machine = new.clone();
            }
            for file in files {
                if file.path == evidence.path() {
                    file.bytes.0 = serde_json::to_vec(&evidence).map_err(|error| error.to_string())?;
                }
            }
        }
        None => {}
    }
    Ok(())
}

fn rename_writer(writer: &mut Writer, old: &MachineName, new: &MachineName) {
    match writer {
        Writer::Resident { address } if address.machine == *old => address.machine = new.clone(),
        Writer::Page { page } => {
            if page.on == *old {
                page.on = new.clone();
            }
            if page.opened_on == *old {
                page.opened_on = new.clone();
            }
            rename_writer(&mut page.by, old, new);
        }
        _ => {}
    }
}

fn rename_act(act: &mut fastctx_types::pages::PageAct, old: &MachineName, new: &MachineName) {
    match act {
        fastctx_types::pages::PageAct::Run { machine, .. } if machine == old => *machine = new.clone(),
        fastctx_types::pages::PageAct::Say { to } if to.machine == *old => to.machine = new.clone(),
        _ => {}
    }
}

fn parse<T: serde::de::DeserializeOwned>(path: &str, bytes: &[u8]) -> Result<T, String> {
    serde_json::from_slice(bytes).map_err(|error| format!("{path}: {error}"))
}

fn pretty(value: &impl serde::Serialize) -> Vec<u8> {
    let mut bytes = serde_json::to_vec_pretty(value).expect("the World's types always serialize");
    bytes.push(b'\n');
    bytes
}
