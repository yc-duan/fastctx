//! Letting go, at the hub operator's word, of the hub's own copies of the
//! large objects the machines conversations live on vouch they keep
//! (`fastctx_content::v2::Library::reclaim_vouched`; design cleanup v3, 1).
//! A file named `SWITCH` in the hub's directory saying `count` has them
//! counted, owner library by owner library, into `SWITCH.json`; saying
//! `go`, let go a batch at a time. The file goes when a round is done, or
//! is taken away — or written anew — to stop one; without it nothing is let
//! go. Both are hidden files, which a move of the hub leaves behind; a batch
//! is let go under the seal, as a grant commits, and as work the hub counts:
//! sealed for a recovery, or stopping to move, nothing more goes.

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::time::Duration;

use serde::Serialize;

use super::Assets;

/// The operator's file, in the hub's directory.
pub(crate) const SWITCH: &str = ".reclaim-vouched-objects";

/// How many large objects one batch goes through.
const BATCH: usize = 256;

/// How many tombstones a round settles of each library before its batches.
const SETTLED_A_ROUND: usize = 1024;

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
    /// Of those let go, the bytes given back to the disk, and how many are
    /// tombstones still, their bytes not deleted yet.
    freed: u64,
    stuck: u64,
    /// Kept, by why: how many, and their bytes.
    kept: BTreeMap<String, (u64, u64)>,
    failed: Vec<String>,
}

/// The operator's file as it was read: what it said, and when it was written.
#[derive(PartialEq)]
struct Asked {
    said: String,
    written: Option<std::time::SystemTime>,
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

impl Assets {
    /// One round of what the operator's file asks, where it is there: what it
    /// came to, said, or nothing where the file is not. The devices that
    /// vouched for an object count only where they may hold its conversation
    /// in `directory` now. Each batch goes as work the hub counts while it
    /// runs (`enter`, none once the hub stops taking anything). Call on a
    /// thread that may block.
    pub(crate) fn reclaim_round(
        &self,
        directory: &fastctx_net::directory_index::Indexed<'_>,
        enter: &dyn Fn() -> Option<crate::moving::Active>,
    ) -> io::Result<Option<String>> {
        let switch = self.root.join(SWITCH);
        let Some(first) = asked(&switch)? else { return Ok(None) };
        let path = self.root.join(format!("{SWITCH}.json"));
        let mut report = Report { asked: first.said.clone(), began: Some(jiff::Timestamp::now()), ..Report::default() };
        let keep = |report: &mut Report| -> io::Result<()> {
            report.at = Some(jiff::Timestamp::now());
            let bytes = serde_json::to_vec_pretty(report).map_err(io::Error::other)?;
            fastctx_platform::publish(&path, &bytes, fastctx_platform::Permissions::Private)
        };
        // Done, the file is taken away only where it is the one this round
        // went by: one written anew meanwhile is the next round's.
        let finish = |report: &mut Report| -> io::Result<Option<String>> {
            report.done = true;
            keep(report)?;
            if asked(&switch)?.as_ref() == Some(&first) {
                match fastctx_platform::disk::remove_file(&switch) {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error),
                }
            }
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
        // Who may hold a scope now, as the directory says: asked once a round.
        let mut judged: BTreeMap<(String, [u8; 32]), bool> = BTreeMap::new();
        let mut may_hold = |voucher: &(String, [u8; 32])| -> bool {
            if let Some(known) = judged.get(voucher) {
                return *known;
            }
            let (scope, key) = voucher;
            let able = directory.all.members.iter().find(|member| member.key.as_bytes() == key).is_some_and(|member| {
                super::scopes::scope(self, scope)
                    .ok()
                    .flatten()
                    .is_some_and(|kept| super::scopes::readable(directory, &member.name, &kept))
            });
            judged.insert(voucher.clone(), able);
            able
        };
        for owner in self.owners.on_disk()? {
            let library = match self.library(&owner) {
                Ok(library) => library,
                Err(error) => {
                    report.failed.push(format!("{}: {error}", owner.key()));
                    continue;
                }
            };
            // What an earlier round could not finish deleting goes first.
            if apply {
                match library.settle_tombstones(SETTLED_A_ROUND) {
                    Ok((_, bytes, stuck)) => {
                        report.freed += bytes;
                        report.stuck += stuck;
                    }
                    Err(error) => report.failed.push(format!("{}: settling what was let go: {error}", owner.key())),
                }
            }
            let mut after = None;
            loop {
                // Taken away or written anew, the hub sealed or moving:
                // stopped here.
                let stop = if asked(&switch)?.as_ref() != Some(&first) {
                    Some("the file was taken away or written anew".to_string())
                } else {
                    self.sealed().map(|why| format!("the hub is sealed for a recovery: {why}"))
                };
                let active = if stop.is_none() { enter() } else { None };
                if stop.is_some() || active.is_none() {
                    report.failed.push(format!("stopped: {}", stop.unwrap_or_else(|| "the hub takes nothing now".into())));
                    keep(&mut report)?;
                    return Ok(Some(format!("vouched objects ({}): stopped", first.said)));
                }
                let able: BTreeSet<(String, [u8; 32])> = match library.vouchers(after, BATCH) {
                    Ok(vouchers) => vouchers.into_iter().filter(|voucher| may_hold(voucher)).collect(),
                    Err(error) => {
                        report.failed.push(format!("{}: {error}", owner.key()));
                        break;
                    }
                };
                // Under the seal, as a grant commits: the hub seals for a
                // recovery only once nothing of this batch is let go.
                let batch = {
                    let seal = self.seal_held();
                    if let Some(why) = seal.as_ref() {
                        report.failed.push(format!("stopped: the hub is sealed for a recovery: {why}"));
                        drop(seal);
                        keep(&mut report)?;
                        return Ok(Some(format!("vouched objects ({}): stopped", first.said)));
                    }
                    library.reclaim_vouched(after, BATCH, fastctx_types::asset_peers::OBJECT_BYTES, apply, able)
                };
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
