//! Letting go, at the hub operator's word, of the hub's own copies of the
//! large objects the machines conversations live on vouch they keep
//! (`fastctx_content::v2::Library::reclaim_vouched`; design cleanup v3, 1).
//! A file named `SWITCH` in the hub's directory saying `count` has them
//! counted, owner library by owner library, into `SWITCH.json`; saying
//! `go`, let go a batch at a time. A round done is kept as done in the
//! report: the file is not gone by again until it is written anew. Taken
//! away — or written anew — a round stops; without it nothing is let go.
//! Both are hidden files, which a move of the hub leaves behind. A library
//! is opened, and what of it is let go deleted, only as work the hub counts:
//! stopping to move, nothing more goes; a batch is let go under the seal, as
//! a grant commits: sealed for a recovery, nothing more goes.

use std::collections::BTreeMap;
use std::io;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::Assets;

/// The operator's file, in the hub's directory.
pub(crate) const SWITCH: &str = ".reclaim-vouched-objects";

/// How many large objects one batch goes through.
const BATCH: usize = 256;

/// How many tombstones are settled at a time.
const SETTLED_A_TIME: usize = 1024;

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
    /// The asking the round was of, once done: it is not gone by again.
    finished: Option<Finished>,
    /// Let go, or that may be: how many, and their bytes.
    objects: u64,
    bytes: u64,
    /// Of those let go, and of what earlier rounds let go, the bytes given
    /// back to the disk, and how many are tombstones still, their bytes not
    /// deleted yet.
    freed: u64,
    stuck: u64,
    /// Kept, by why: how many, and their bytes.
    kept: BTreeMap<String, (u64, u64)>,
    failed: Vec<String>,
}

/// An asking a round went by to its end: what the file said, and when it was
/// written, in nanoseconds.
#[derive(Serialize, Deserialize, PartialEq, Clone)]
struct Finished {
    said: String,
    written: Option<u128>,
}

/// What a report says was done, read back.
#[derive(Deserialize)]
struct Done {
    #[serde(default)]
    finished: Option<Finished>,
}

/// The operator's file as it was read: what it said, and when it was written.
#[derive(PartialEq)]
struct Asked {
    said: String,
    written: Option<std::time::SystemTime>,
}

impl Asked {
    fn finished(&self) -> Finished {
        Finished {
            said: self.said.clone(),
            written: self.written.and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok()).map(|since| since.as_nanos()),
        }
    }
}

fn asked(switch: &std::path::Path) -> io::Result<Option<Asked>> {
    let bytes = match fastctx_platform::disk::read(switch) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let written = fastctx_platform::disk::metadata(switch).ok().and_then(|found| found.modified().ok());
    Ok(Some(Asked { said: String::from_utf8_lossy(&bytes).trim().to_string(), written }))
}

/// The asking the report beside the file says was gone by to its end.
fn finished(report: &std::path::Path) -> Option<Finished> {
    let bytes = fastctx_platform::disk::read(report).ok()?;
    serde_json::from_slice::<Done>(&bytes).ok()?.finished
}

impl Assets {
    /// One round of what the operator's file asks, where it is there and was
    /// not gone by to its end already: what it came to, said, or nothing.
    /// The devices that vouched for an object count only where they may hold
    /// its conversation in the hub's directory as it stands as each batch is
    /// judged (`directory`, none once the hub is gone), and read it back
    /// since they came to the network as they are now. Each library is
    /// opened, and each batch goes, as work the hub counts while it runs
    /// (`enter`, none once the hub stops taking anything). Call on a thread
    /// that may block.
    pub(crate) fn reclaim_round(
        &self,
        directory: &dyn Fn() -> Option<Arc<crate::store::Snapshot>>,
        enter: &dyn Fn() -> Option<crate::moving::Active>,
    ) -> io::Result<Option<String>> {
        let switch = self.root.join(SWITCH);
        let Some(first) = asked(&switch)? else { return Ok(None) };
        let path = self.root.join(format!("{SWITCH}.json"));
        if finished(&path).as_ref() == Some(&first.finished()) {
            return Ok(None);
        }
        let mut report = Report { asked: first.said.clone(), began: Some(jiff::Timestamp::now()), ..Report::default() };
        let keep = |report: &mut Report| -> io::Result<()> {
            report.at = Some(jiff::Timestamp::now());
            let bytes = serde_json::to_vec_pretty(report).map_err(io::Error::other)?;
            fastctx_platform::publish(&path, &bytes, fastctx_platform::Permissions::Private)
        };
        let finish = |report: &mut Report| -> io::Result<Option<String>> {
            report.done = true;
            report.finished = Some(first.finished());
            keep(report)?;
            Ok(Some(format!(
                "vouched objects ({}): {} objects, {} bytes{}, {} bytes given back, {} not deleted yet{}",
                report.asked,
                report.objects,
                report.bytes,
                if report.asked == "go" { " let go" } else { " may be let go" },
                report.freed,
                report.stuck,
                if report.failed.is_empty() { String::new() } else { format!("; {}", report.failed.join("; ")) }
            )))
        };
        let apply = match first.said.as_str() {
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
        // Taken away or written anew, the hub sealed or moving: stopped
        // there, said so.
        let stopped = |report: &mut Report, why: String| -> io::Result<Option<String>> {
            report.failed.push(format!("stopped: {why}"));
            keep(report)?;
            Ok(Some(format!("vouched objects ({}): stopped: {why}", first.said)))
        };
        let go_on = || -> io::Result<Result<crate::moving::Active, String>> {
            if asked(&switch)?.as_ref() != Some(&first) {
                return Ok(Err("the file was taken away or written anew".into()));
            }
            if let Some(why) = self.sealed() {
                return Ok(Err(format!("the hub is sealed for a recovery: {why}")));
            }
            Ok(enter().ok_or_else(|| "the hub takes nothing now".to_string()))
        };
        for owner in self.owners.on_disk()? {
            // Opened as work the hub counts.
            let library = {
                let _active = match go_on()? {
                    Ok(active) => active,
                    Err(why) => return stopped(&mut report, why),
                };
                match self.library(&owner) {
                    Ok(library) => library,
                    Err(error) => {
                        report.failed.push(format!("{}: {error}", owner.key()));
                        continue;
                    }
                }
            };
            // What earlier rounds could not finish deleting goes first, a page
            // at a time, each as work the hub counts while the file asks it.
            if apply {
                let mut after = None;
                loop {
                    let _active = match go_on()? {
                        Ok(active) => active,
                        Err(why) => return stopped(&mut report, why),
                    };
                    let seal = self.seal_held();
                    if let Some(why) = seal.as_ref() {
                        let why = format!("the hub is sealed for a recovery: {why}");
                        drop(seal);
                        return stopped(&mut report, why);
                    }
                    match library.settle_tombstones(after, SETTLED_A_TIME) {
                        Ok(settled) => {
                            report.freed += settled.bytes;
                            report.stuck += settled.stuck;
                            after = settled.next;
                        }
                        Err(error) => {
                            report.failed.push(format!("{}: settling what was let go: {error}", owner.key()));
                            after = None;
                        }
                    }
                    drop(seal);
                    if after.is_none() {
                        break;
                    }
                }
            }
            let mut after = None;
            loop {
                let active = match go_on()? {
                    Ok(active) => active,
                    Err(why) => return stopped(&mut report, why),
                };
                let Some(now) = directory() else { return stopped(&mut report, "the hub is gone".into()) };
                // Under the seal, then the directory's writer, as a grant
                // takes them: who may hold each scope, and since when each
                // device is on the network as it is now, is judged as the
                // directory stands while the batch is marked, no change of it
                // coming between; the bytes go after, outside both.
                let marked = {
                    let seal = self.seal_held();
                    if let Some(why) = seal.as_ref() {
                        let why = format!("the hub is sealed for a recovery: {why}");
                        drop(seal);
                        return stopped(&mut report, why);
                    }
                    let marked = self.bodies.under_directory(&now.book.directory, |current| -> Result<fastctx_content::v2::Marked, String> {
                        let index = fastctx_net::directory_index::Index::new(current);
                        let indexed = fastctx_net::directory_index::Indexed::new(current, &index);
                        let able: fastctx_content::v2::Able = library
                            .vouchers(after, BATCH)
                            .map_err(|error| error.to_string())?
                            .into_iter()
                            .filter_map(|voucher| {
                                let (scope, key) = &voucher;
                                let member = indexed.all.members.iter().find(|member| member.key.as_bytes() == key)?;
                                let kept = super::scopes::scope(self, scope).ok().flatten()?;
                                super::scopes::readable(&indexed, &member.name, &kept)
                                    .then(|| (voucher.clone(), member.added.as_millisecond()))
                            })
                            .collect();
                        library
                            .mark_vouched(after, BATCH, fastctx_types::asset_peers::OBJECT_BYTES, apply, able)
                            .map_err(|error| error.to_string())
                    });
                    drop(seal);
                    marked
                };
                drop(now);
                let batch = marked.map(|marked| {
                    let next = marked.next;
                    (next, library.let_go_marked(marked))
                });
                drop(active);
                match batch {
                    Ok((next, found)) => {
                        report.objects += found.objects;
                        report.bytes += found.bytes;
                        report.freed += found.freed;
                        report.stuck += found.stuck;
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
