//! Display text has a wire representation of bounded size; identities stay exact.

use fastctx_types::residents::Origin;
use fastctx_types::town::{Afoot, AfootChange, AfootJob, AfootResident, ResidentNow, RunningJob};

const TEXT: usize = 4096;
const SHORT: &str = " … [shortened; open the full record]";

fn shorten(text: &mut String, limit: usize) {
    if text.len() <= limit {
        return;
    }
    let mut end = limit.saturating_sub(SHORT.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
    text.push_str(SHORT);
}

fn optional(text: &mut Option<String>) {
    if let Some(text) = text {
        shorten(text, TEXT);
    }
}

fn resident(resident: &mut ResidentNow) {
    shorten(&mut resident.about.host.client, TEXT);
    shorten(&mut resident.about.host.version, TEXT);
    if let Origin::Colleague { label, .. } = &mut resident.about.origin {
        shorten(label, TEXT);
    }
    if let Some(now) = &mut resident.now {
        optional(&mut now.title);
        optional(&mut now.first_words);
        optional(&mut now.last_said);
        if let Some(trouble) = &mut now.trouble {
            shorten(&mut trouble.text, TEXT);
        }
        if let Some(asked) = &mut now.asked {
            shorten(&mut asked.text, TEXT);
            if asked.options.len() > 32 || asked.options.iter().map(String::len).sum::<usize>() > TEXT {
                // Shortening an answer would change what choosing it sends.
                asked.options.clear();
                asked.text.push_str(" [Open the conversation to see and choose the complete answers.]");
            }
        }
    }
}

fn job(job: &mut RunningJob) {
    shorten(&mut job.command, TEXT);
}

fn fits(value: &impl serde::Serialize) -> Result<(), String> {
    let size = serde_json::to_vec(value).map_err(|error| error.to_string())?.len();
    if size + 256 > super::FRAME_ROOM {
        return Err(format!(
            "a status record needs {size} bytes even with its display text shortened; its identity and metadata cannot fit in one control message"
        ));
    }
    Ok(())
}

pub fn state(afoot: &mut Afoot) -> Result<(), String> {
    for entry in afoot.residents.values_mut() {
        resident(&mut entry.resident);
        fits(entry)?;
    }
    for entry in afoot.jobs.values_mut() {
        job(&mut entry.job);
        fits(entry)?;
    }
    Ok(())
}

pub fn change(change: &mut AfootChange<AfootResident, AfootJob>) -> Result<(), String> {
    for entry in &mut change.residents {
        resident(&mut entry.resident);
        fits(entry)?;
    }
    for entry in &mut change.jobs {
        job(&mut entry.job);
        fits(entry)?;
    }
    Ok(())
}
