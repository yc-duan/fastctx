//! The jobs a conversation started, in its scope of the asset library
//! (design §13, plan 3-4 group 3): each job's facts as its record stands,
//! and its output a piece at a time — what came since, as an object of its
//! bytes — then, once the job ended and every byte is written, its end.
//! Looked at every few seconds among the jobs running or ended within the
//! day; only what is new is written. Made by the machine the job runs on,
//! for a conversation that lives here and keeps its records with an account
//! on a network: there the conversation's scope is ordered here. A job's
//! files go, as their retention says, only once its output is written and
//! the hub took it (`kept_for_good`).

use crate::Node;
use fastctx_content::v2::{JobFacts, JobOutput, Kind as _, LEASE, Library, Write, Writer};
use fastctx_types::ResidentId;
use fastctx_types::run::{JobOwner, JobRecord};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{Read as _, Seek as _, SeekFrom};
use std::sync::{Arc, Weak};
use std::time::Duration;

/// How often the jobs here are looked at.
const EVERY: Duration = Duration::from_secs(10);

/// The most bytes a piece of output holds, and one job writes in a look.
const PIECE: u64 = 1 << 20;
const A_LOOK: u64 = 8 << 20;

/// The largest record of a job's facts.
const FACTS_BYTES: usize = 60 * 1024;

/// Where a job's assets stand, kept beside them.
#[derive(Default, Serialize, Deserialize)]
struct Standing {
    /// The digest of the facts written last.
    facts: String,
    /// How much of each part is written.
    output: u64,
    stderr: u64,
    /// Every byte written and its end too.
    done: bool,
}

fn checkpoint(job: &JobRecord) -> String {
    format!("job/{}", job.id)
}

/// The conversation of this machine `job` was started by.
fn resident_of(node: &Node, job: &JobRecord) -> Option<ResidentId> {
    match &job.owner {
        JobOwner::Resident(address) if node.is_me(&address.machine) => Some(address.resident.clone()),
        _ => None,
    }
}

/// Starts looking at the jobs here every few seconds.
pub(crate) fn start(node: &Arc<Node>) {
    let weak: Weak<Node> = Arc::downgrade(node);
    fastctx_platform::task::spawn(async move {
        loop {
            tokio::time::sleep(EVERY).await;
            let Some(node) = weak.upgrade() else { return };
            if node.assets.is_none() {
                continue;
            }
            let working = node.clone();
            if let Err(error) = fastctx_platform::task::blocking(move || look(&working)).await {
                node.log.line(format_args!("writing the jobs here into the asset library: {error}"));
            }
        }
    });
}

/// One look at the jobs running or ended within the day.
fn look(node: &Arc<Node>) -> Result<(), String> {
    let since = jiff::Timestamp::now() - jiff::SignedDuration::from_hours(24);
    let jobs = crate::job_index::recent(&node.world, since).map_err(|error| error.to_string())?;
    let mut failed = None;
    for (job, _) in jobs {
        let Some(resident) = resident_of(node, &job) else { continue };
        if let Err(error) = write_job(node, &resident, &job) {
            failed.get_or_insert(format!("{}: {error}", job.id));
        }
    }
    failed.map_or(Ok(()), Err)
}

/// The scope `resident`'s jobs go in, when this machine orders its job
/// streams there.
fn writable(node: &Arc<Node>, library: &Library, resident: &ResidentId) -> Result<Option<(String, i64)>, String> {
    let Some((hub, scope)) = crate::timeline::scope_of(node, resident).map_err(|failure| failure.message)? else {
        return Ok(None);
    };
    let Some(kept) = node.assets.as_ref() else { return Ok(None) };
    if crate::assets::release::released_away(node.world.conversation(resident).path(), &scope, &kept.device()) {
        return Ok(None);
    }
    let error = |error: fastctx_content::Error| error.to_string();
    // Begun by the conversation's timeline or words, as theirs is; one from
    // elsewhere not before the hub described it (AG10).
    let Some(holding) = library.holding(&scope).map_err(error)? else { return Ok(None) };
    let born_elsewhere = node.residents.birth_machine(resident).is_some_and(|born| !node.is_me(&born));
    if born_elsewhere && !library.is_live(&scope).map_err(error)? {
        return Ok(None);
    }
    for stream in [JobFacts::STREAM, JobOutput::STREAM] {
        if library.orderer(&scope, stream).map_err(error)?.is_some_and(|orderer| orderer != kept.device()) {
            crate::assets::takeover::want(node, &hub, &scope, stream);
            return Ok(None);
        }
    }
    Ok(Some((scope, holding)))
}

/// Writes what is new of `job`, started by `resident`.
fn write_job(node: &Arc<Node>, resident: &ResidentId, job: &JobRecord) -> Result<(), String> {
    let Some(kept) = node.assets.as_ref() else { return Ok(()) };
    let library = &kept.library;
    let error = |error: fastctx_content::Error| error.to_string();
    let name = checkpoint(job);
    let mut standing: Standing = library
        .checkpoint(&name)
        .map_err(error)?
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    if standing.done {
        return Ok(());
    }
    let Some((scope, holding)) = writable(node, library, resident)? else { return Ok(()) };
    let signer = kept.signer();
    let mut changed = false;
    // Its facts, as its record stands.
    let mut facts = serde_json::to_value(job).map_err(|error| error.to_string())?;
    if serde_json::to_vec(&facts).map_or(0, |bytes| bytes.len()) > FACTS_BYTES {
        facts["script"] = serde_json::json!({ "omitted": "too large", "bytes": job.script.len() });
    }
    let facts = serde_json::to_vec(&facts).map_err(|error| error.to_string())?;
    let digest: String = Sha256::digest(&facts).iter().map(|byte| format!("{byte:02x}")).collect();
    if digest != standing.facts {
        let item = job.id.to_string().into_bytes();
        Writer::<JobFacts>::new(library, &scope, JobFacts::STREAM)
            .map_err(error)?
            .in_holding(holding)
            .write_all(
                vec![Write { order: item.clone(), item, value: Some(facts), objects: Vec::new() }],
                Vec::new(),
                false,
                signer.clone(),
            )
            .map_err(error)?;
        standing.facts = digest;
        changed = true;
    }
    // Its output, what came since.
    let output = Writer::<JobOutput>::new(library, &scope, JobOutput::STREAM).map_err(error)?.in_holding(holding);
    let mut budget = A_LOOK;
    let mut whole = true;
    for (part, path) in [("output", Some(job.output.as_str())), ("stderr", job.stderr.as_deref())] {
        let Some(path) = path else { continue };
        let length = fastctx_platform::disk::metadata(path).map(|metadata| metadata.len()).unwrap_or(0);
        let at = if part == "output" { &mut standing.output } else { &mut standing.stderr };
        if *at >= length {
            continue;
        }
        let mut file = fastctx_platform::disk::open(path).map_err(|error| error.to_string())?;
        file.seek(SeekFrom::Start(*at)).map_err(|error| error.to_string())?;
        while *at < length && budget > 0 {
            let take = (length - *at).min(PIECE).min(budget);
            let mut bytes = Vec::with_capacity(take as usize);
            (&mut file).take(take).read_to_end(&mut bytes).map_err(|error| error.to_string())?;
            if bytes.is_empty() {
                break;
            }
            let lease = library.put(&mut bytes.as_slice(), "job-output", LEASE).map_err(error)?;
            let item = format!("{}/{part}/{:016x}", job.id, *at).into_bytes();
            let body = serde_json::to_vec(
                &serde_json::json!({ "job": job.id, "part": part, "offset": *at, "length": bytes.len() }),
            )
            .map_err(|error| error.to_string())?;
            output
                .write_all(
                    vec![Write { order: item.clone(), item, value: Some(body), objects: vec![lease.object] }],
                    vec![lease],
                    false,
                    signer.clone(),
                )
                .map_err(error)?;
            *at += bytes.len() as u64;
            budget = budget.saturating_sub(bytes.len() as u64);
            changed = true;
        }
        whole &= *at >= length;
    }
    // Ended, every byte drained and written: its end, once.
    let drained = job.output_ended_at.is_some() || job.output_drained.is_some();
    if job.end.is_some() && drained && whole {
        let item = format!("{}/end", job.id).into_bytes();
        let body = serde_json::to_vec(&serde_json::json!({
            "job": job.id,
            "end": true,
            "output": standing.output,
            "stderr": standing.stderr,
        }))
        .map_err(|error| error.to_string())?;
        output
            .write_all(
                vec![Write { order: item.clone(), item, value: Some(body), objects: Vec::new() }],
                Vec::new(),
                false,
                signer,
            )
            .map_err(error)?;
        standing.done = true;
        changed = true;
    }
    if changed {
        library
            .keep_checkpoint(&name, serde_json::to_vec(&standing).map_err(|error| error.to_string())?)
            .map_err(error)?;
    }
    Ok(())
}

/// Whether `job`'s files may go as their retention says: its output is
/// written into the asset library whole and the hub took it — or it keeps
/// no assets here at all. Call on a thread that may block.
pub(crate) fn kept_for_good(node: &Node, job: &JobRecord) -> bool {
    let Some(kept) = node.assets.as_ref() else { return true };
    let Some(resident) = resident_of(node, job) else { return true };
    let Ok(Some((_, scope))) = crate::timeline::scope_of(node, &resident) else { return true };
    let library = &kept.library;
    let standing = library.checkpoint(&checkpoint(job)).ok().flatten().and_then(|bytes| serde_json::from_slice::<Standing>(&bytes).ok());
    match standing {
        Some(standing) if standing.done => {}
        Some(_) => return false,
        // Never begun, and ended before the day the jobs here are looked at
        // in — a job of before this release — it goes as it always did.
        None => {
            let since = jiff::Timestamp::now() - jiff::SignedDuration::from_hours(24);
            return job.end.as_ref().is_some_and(|end| end.at < since);
        }
    }
    // Every frame of the job streams here went up.
    library
        .uploads(&scope, &kept.device())
        .is_ok_and(|uploads| uploads.iter().all(|(stream, _)| stream != JobOutput::STREAM && stream != JobFacts::STREAM))
}
