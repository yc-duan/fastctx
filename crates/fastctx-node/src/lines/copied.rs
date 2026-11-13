//! A job's output kept on another machine, read from the copy here (plan
//! 3-5, design §28.27): every device that holds the job's conversation holds
//! its output as pieces of the `output` stream, one object each, and its end
//! once it ended. Read from here when the copy has the part whole — its end
//! says nothing of it is missing and the pieces reach its length — or when
//! the machine it ran on is away, as far as the pieces run on unbroken.
//! Written out under the world's `passing/` for as long as it is read, and
//! opened there as this machine's file.

use crate::Node;
use crate::attachments::kept_here::Passing;
use fastctx_content::v2::{JobFacts, JobOutput, Kind as _};
use fastctx_types::{JobId, MachineName};
use std::sync::Arc;

/// The bytes of one part of a job's output most read out at a time.
const READ: u32 = 1 << 20;

/// The output at `path` of `job`, run on `machine`, from the copy here:
/// whole, or what came of it while the machine is away; none otherwise, the
/// machine itself asked then.
pub(crate) async fn output(node: &Arc<Node>, machine: &MachineName, job: &JobId, path: &str) -> Option<Passing> {
    let library = node.assets.as_ref()?.library.clone();
    let away = crate::timeline::copy::away(node, machine);
    let asking = node.clone();
    let (job, path, machine) = (job.clone(), path.to_string(), machine.clone());
    let folder = node.world.path().join("passing");
    fastctx_platform::task::blocking(move || -> Option<Passing> {
        // The one scope read here that holds the job: two that do say
        // nothing of which (as the record's own copy does, §28.34).
        let mut found = None;
        for scope in library.held_scopes().ok()? {
            if !crate::timeline::copy::read_here(&asking, &scope) {
                continue;
            }
            let Ok(Some(entry)) = library.item(&scope.key, JobFacts::STREAM, job.as_str().as_bytes()) else { continue };
            if found.replace((scope.key.clone(), entry)).is_some() {
                return None;
            }
        }
        let (scope, entry) = found?;
        let record: serde_json::Value = serde_json::from_slice(&entry.body).ok()?;
        if record.get("machine").and_then(serde_json::Value::as_str) != Some(machine.as_str()) {
            return None;
        }
        let part = if record.get("output").and_then(serde_json::Value::as_str) == Some(path.as_str()) {
            "output"
        } else if record.get("stderr").and_then(serde_json::Value::as_str) == Some(path.as_str()) {
            "stderr"
        } else {
            return None;
        };
        let end: Option<serde_json::Value> = library
            .item(&scope, JobOutput::STREAM, format!("{job}/end").as_bytes())
            .ok()?
            .and_then(|end| serde_json::from_slice(&end.body).ok());
        let length = end.as_ref().and_then(|end| {
            let lost = end.get("missing").and_then(serde_json::Value::as_array).is_some_and(|missing| {
                missing.iter().any(|lost| lost.as_str() == Some(part))
            });
            (!lost).then(|| end.get(part).and_then(serde_json::Value::as_u64)).flatten()
        });
        if length.is_none() && !away {
            return None;
        }
        fastctx_platform::disk::create_dir_all(&folder).ok()?;
        let path = folder.join(format!("output-{}-{part}-{}", job, std::process::id() as u64 ^ jiff::Timestamp::now().as_nanosecond() as u64));
        let passing = Passing::new(path.clone());
        let mut out = fastctx_platform::disk::create(&path).ok()?;
        let prefix = format!("{job}/{part}/");
        let mut after = Some((prefix.clone().into_bytes(), Vec::new()));
        let mut at = 0u64;
        'pieces: loop {
            let page = library.page(&scope, JobOutput::STREAM, after.clone(), 500).ok()?;
            let Some(last) = page.last() else { break };
            after = Some((last.order.clone(), last.item.clone()));
            let full = page.len() == 500;
            for piece in page {
                if !piece.item.starts_with(prefix.as_bytes()) {
                    break 'pieces;
                }
                let body: serde_json::Value = serde_json::from_slice(&piece.body).ok()?;
                let (offset, size) = (body.get("offset")?.as_u64()?, body.get("length")?.as_u64()?);
                let [object] = piece.objects.as_slice() else { return None };
                // A piece past a gap: what runs on unbroken is all there is.
                if offset != at || object.size != size || library.size(&object.hash).ok()? != Some(size) {
                    break 'pieces;
                }
                let mut read = 0u64;
                while read < size {
                    let bytes = library.read(&object.hash, read, READ).ok()?;
                    if bytes.is_empty() {
                        return None;
                    }
                    std::io::Write::write_all(&mut out, &bytes).ok()?;
                    read += bytes.len() as u64;
                }
                at += size;
            }
            if !full {
                break;
            }
        }
        // Said whole, it is read from here only when it is.
        if let Some(length) = length
            && at != length
            && !away
        {
            return None;
        }
        Some(passing)
    })
    .await
}

