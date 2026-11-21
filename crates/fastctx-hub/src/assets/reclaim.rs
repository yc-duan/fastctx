//! Letting go, at the hub operator's word, of the hub's own copies of the
//! large objects the machines conversations live on vouch they keep
//! (`fastctx_content::v2::Library::reclaim_vouched`; design cleanup v3, 1).
//! A file named `SWITCH` in the hub's directory saying `count` has them
//! counted, owner library by owner library, into `SWITCH.json`; saying
//! `go`, let go a batch at a time. The file goes when a round is done, or
//! is taken away to stop one; without it nothing is let go. Both are hidden
//! files, which a move of the hub leaves behind; a round stops as the hub
//! is sealed for a recovery or stops taking anything to move.

use std::collections::BTreeMap;
use std::io;
use std::time::Duration;

use serde::Serialize;

use super::Assets;

/// The operator's file, in the hub's directory.
pub(crate) const SWITCH: &str = ".reclaim-vouched-objects";

/// How many large objects one batch goes through.
const BATCH: usize = 256;

/// How long letting go waits between two batches: the library's writer is
/// the hub's, and the hub's work goes first.
const BREATHE: Duration = Duration::from_millis(200);

/// What a round came to, kept beside the operator's file as it goes.
#[derive(Serialize, Default)]
struct Report {
    asked: String,
    began: Option<jiff::Timestamp>,
    at: Option<jiff::Timestamp>,
    /// The owner libraries gone through.
    owners: usize,
    done: bool,
    /// Let go, or that may be: how many, and their bytes.
    objects: u64,
    bytes: u64,
    /// Kept, by why: how many, and their bytes.
    kept: BTreeMap<String, (u64, u64)>,
    failed: Vec<String>,
}

impl Assets {
    /// One round of what the operator's file asks, where it is there: what it
    /// came to, said, or nothing where the file is not. Each batch goes as
    /// work the hub counts while it runs (`enter`, none once the hub stops
    /// taking anything). Call on a thread that may block.
    pub(crate) fn reclaim_round(&self, enter: &dyn Fn() -> Option<crate::moving::Active>) -> io::Result<Option<String>> {
        let switch = self.root.join(SWITCH);
        let asked = match fastctx_platform::disk::read(&switch) {
            Ok(bytes) => String::from_utf8_lossy(&bytes).trim().to_string(),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let path = self.root.join(format!("{SWITCH}.json"));
        let mut report = Report { asked: asked.clone(), began: Some(jiff::Timestamp::now()), ..Report::default() };
        let keep = |report: &mut Report| -> io::Result<()> {
            report.at = Some(jiff::Timestamp::now());
            let bytes = serde_json::to_vec_pretty(report).map_err(io::Error::other)?;
            fastctx_platform::publish(&path, &bytes, fastctx_platform::Permissions::Private)
        };
        let finish = |report: &mut Report| -> io::Result<Option<String>> {
            report.done = true;
            keep(report)?;
            match fastctx_platform::disk::remove_file(&switch) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
            Ok(Some(format!(
                "vouched objects ({}): {} objects, {} bytes{}{}",
                report.asked,
                report.objects,
                report.bytes,
                if report.asked == "go" { " let go" } else { " may be let go" },
                if report.failed.is_empty() { String::new() } else { format!("; {}", report.failed.join("; ")) }
            )))
        };
        let apply = match asked.as_str() {
            "count" => false,
            "go" => true,
            _ => {
                report.failed.push("the file says neither `count` nor `go`".into());
                return finish(&mut report);
            }
        };
        // Through the hub, a conversation's streams are the hub's to keep:
        // its copies are the copies.
        if self.sync_version() < fastctx_types::assets::VOUCH {
            report.failed.push(format!("conversations come through this hub ({}): nothing is let go", super::THROUGH_HUB));
            return finish(&mut report);
        }
        if let Some(why) = self.sealed() {
            report.failed.push(format!("the hub is sealed for a recovery: {why}"));
            return finish(&mut report);
        }
        keep(&mut report)?;
        for owner in self.owners.on_disk()? {
            let library = match self.library(&owner) {
                Ok(library) => library,
                Err(error) => {
                    report.failed.push(format!("{}: {error}", owner.key()));
                    continue;
                }
            };
            let mut after = None;
            loop {
                // Taken away, the hub sealed or moving: stopped here.
                let stop = if !fastctx_platform::disk::is_there(&switch) {
                    Some("the file was taken away".to_string())
                } else {
                    self.sealed().map(|why| format!("the hub is sealed for a recovery: {why}"))
                };
                let active = if stop.is_none() { enter() } else { None };
                if stop.is_some() || active.is_none() {
                    report.failed.push(format!("stopped: {}", stop.unwrap_or_else(|| "the hub takes nothing now".into())));
                    keep(&mut report)?;
                    return Ok(Some(format!("vouched objects ({asked}): stopped")));
                }
                let batch = library.reclaim_vouched(after, BATCH, fastctx_types::asset_peers::OBJECT_BYTES, apply);
                drop(active);
                match batch {
                    Ok((next, found)) => {
                        report.objects += found.objects;
                        report.bytes += found.bytes;
                        for (why, (count, bytes)) in found.kept {
                            let entry = report.kept.entry(why.to_string()).or_default();
                            entry.0 += count;
                            entry.1 += bytes;
                        }
                        after = next;
                    }
                    Err(error) => {
                        report.failed.push(format!("{}: {error}", owner.key()));
                        break;
                    }
                }
                if after.is_none() {
                    break;
                }
                if apply {
                    keep(&mut report)?;
                    std::thread::sleep(BREATHE);
                }
            }
            report.owners += 1;
        }
        finish(&mut report)
    }
}
