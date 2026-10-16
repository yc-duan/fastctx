//! Display text has a wire representation of bounded size; identities stay exact.

use fastctx_types::residents::Origin;
use fastctx_types::town::{Afoot, AfootChange, AfootJob, AfootResident, ResidentNow, RunningJob};

const TEXT: usize = 4096;
const SHORT: &str = "…";

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
            }
        }
    }
}

fn job(job: &mut RunningJob) {
    shorten(&mut job.command, TEXT);
}

pub(super) fn fits(value: &impl serde::Serialize) -> Result<(), String> {
    struct Size(usize);
    impl std::io::Write for Size {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > super::FRAME_ROOM - 256 - self.0 {
                return Err(std::io::Error::other("status identity and metadata exceed one control frame"));
            }
            self.0 += bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    serde_json::to_writer(&mut Size(0), value).map_err(|error| error.to_string())
}

pub fn state(afoot: &mut Afoot) {
    for entry in afoot.residents.values_mut() {
        resident(&mut entry.resident);
    }
    for entry in afoot.jobs.values_mut() {
        job(&mut entry.job);
    }
}

pub fn change(change: &mut AfootChange<AfootResident, AfootJob>) {
    for entry in &mut change.residents {
        resident(&mut entry.resident);
    }
    for entry in &mut change.jobs {
        job(&mut entry.job);
    }
}
