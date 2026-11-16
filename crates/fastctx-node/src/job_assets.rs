//! The jobs run here, in the asset library (design §13, §28.58, plan 3-4
//! group 3): each job's facts as its record stands, and its output a piece
//! at a time — what came since, as an object of its bytes — then, once the
//! job ended and every byte is written, its end. Looked at every few
//! seconds: the jobs running or ended within the day, and every one begun
//! here and not finished since, however long ago; only what is new is
//! written, each piece named by where it begins, so a piece written before a
//! stop is not written again. Made by the machine the job runs on: a job of
//! a conversation that lives here goes into the conversation's scope, which
//! is ordered here; one run here for a conversation elsewhere (an `on`), or
//! for a page, into the records of the account it ran for, in streams this
//! machine names after itself. The asset library is where a job is kept for
//! good and read from elsewhere; its files here go, as their retention says,
//! only once its output is written and the hub took it (`kept_for_good`).

use crate::Node;
use fastctx_content::v2::{JobFacts, JobOutput, Kind as _, LEASE, Library, Write, Writer};
use fastctx_types::ResidentId;
use fastctx_types::run::{JobOwner, JobRecord};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::PathBuf;
use std::sync::{Arc, Weak};
use std::time::Duration;

/// How often the jobs here are looked at.
const EVERY: Duration = Duration::from_secs(10);

/// The most bytes a piece of output holds, and each part of one job writes
/// in a look: neither part starves the other (K11).
const PIECE: u64 = 1 << 20;
const A_PART: u64 = 4 << 20;

/// How long after a job ended its output is taken as whole though the
/// supervisor never said it drained — stopped before it could (MM22).
const SETTLED: Duration = Duration::from_secs(60);

/// The largest record of a job's facts.
const FACTS_BYTES: usize = 60 * 1024;

/// What the jobs begun here and not finished are kept under, by job, with
/// where their records are; and since when the jobs here are written at all.
const OPEN: &str = "jobs/open";
const SINCE: &str = "jobs/since";

/// Where a job's assets stand, kept beside them.
#[derive(Default, Serialize, Deserialize)]
struct Standing {
    /// The holding period they were written in: held anew, they are written
    /// again whole (MM20).
    #[serde(default)]
    holding: i64,
    /// The digest of the facts written last.
    facts: String,
    /// How much of each part is written.
    output: u64,
    stderr: u64,
    /// Every byte written and its end too.
    done: bool,
}

/// A job's ids are unique within the folder of what it ran for alone: the
/// key it is kept under here, and in an account's records, names that too —
/// what it ran for, and when it began, as a digest (§28.58).
fn key_of(job: &JobRecord) -> String {
    let mut digest = Sha256::new();
    digest.update(job.owner.tag().as_bytes());
    digest.update(b"\n");
    digest.update(job.started_at.to_string().as_bytes());
    let named: String = digest.finalize().iter().take(6).map(|byte| format!("{byte:02x}")).collect();
    format!("{}~{named}", job.id)
}

fn checkpoint(job: &JobRecord) -> String {
    format!("job/{}", key_of(job))
}

/// What `job` is named by in the streams at `home`: its id in its
/// conversation's own, where ids are unique; its key in an account's
/// records, which hold the jobs of many.
fn item_of(home: &Home, job: &JobRecord) -> String {
    match home {
        Home::Conversation(_) => job.id.to_string(),
        Home::Account(_) => key_of(job),
    }
}

/// When the account `account` was opened, as the hub this machine is on
/// tells it; none when it is not known here now.
fn opened_of(node: &Node, account: &fastctx_types::access::HubAccount) -> Option<jiff::Timestamp> {
    node.link_of(&account.hub)
        .and_then(|link| link.view().directory.account(&account.account).map(|found| found.opened))
}

/// Whether `job` ran for `account` as it is now: one begun before it was
/// opened, or before this machine first saw it opened so, ran for an
/// account of that name before it, not for this one
/// (`assets::activity::of_this`).
fn ran_for(node: &Node, account: &fastctx_types::access::HubAccount, job: &JobRecord) -> bool {
    crate::assets::activity::of_this(node, &account.hub, &account.account, job.started_at.as_millisecond())
}

/// Where a job's assets go.
enum Home {
    /// The scope of its conversation, which lives here.
    Conversation(ResidentId),
    /// The records of the account it ran for: a job run here for a
    /// conversation elsewhere, or for a page.
    Account(fastctx_types::access::HubAccount),
}

/// Where `job`'s assets go; none for one that ran for no account here.
fn home_of(node: &Node, job: &JobRecord) -> Option<Home> {
    match &job.owner {
        JobOwner::Resident(address) if node.is_me(&address.machine) => {
            Some(Home::Conversation(address.resident.clone()))
        }
        _ => job.on.as_ref().and_then(|on| on.account.clone()).or_else(|| job.account.clone()).map(Home::Account),
    }
}

/// The streams of a job's facts and output at `home`: the conversation's
/// own, or those this machine names after itself in an account's records.
fn streams_of(node: &Node, home: &Home) -> (String, String) {
    match home {
        Home::Conversation(_) => (JobFacts::STREAM.to_string(), JobOutput::STREAM.to_string()),
        Home::Account(_) => {
            let named = node.assets.as_ref().map(|assets| crate::assets::named(&assets.device())).unwrap_or_default();
            (format!("{}/{named}", JobFacts::STREAM), format!("{}/{named}", JobOutput::STREAM))
        }
    }
}

/// Starts looking at the jobs here every few seconds.
pub(crate) fn start(node: &Arc<Node>) {
    // Since when the jobs here are written at all, kept as the node starts
    // — before any job of this release can end — so none is taken for one
    // of before it (L04).
    if let Some(kept) = node.assets.as_ref()
        && kept.library.checkpoint(SINCE).is_ok_and(|since| since.is_none())
        && let Err(error) = kept.library.keep_checkpoint(SINCE, jiff::Timestamp::now().to_string().into_bytes())
    {
        node.log.line(format_args!("keeping since when the jobs here are written: {error}"));
    }
    let weak: Weak<Node> = Arc::downgrade(node);
    fastctx_platform::task::spawn(async move {
        // The first look takes in every job since then, as does one after a
        // scope is held anew (NN07, L04).
        let mut whole = true;
        let mut held = 0u64;
        loop {
            let Some(node) = weak.upgrade() else { return };
            if node.assets.is_some() {
                let working = node.clone();
                let looked = fastctx_platform::task::blocking(move || look(&working, whole, held)).await;
                match looked {
                    Ok(now) => {
                        whole = false;
                        held = now;
                    }
                    Err(error) => node.log.line(format_args!("writing the jobs here into the asset library: {error}")),
                }
            }
            drop(node);
            tokio::time::sleep(EVERY).await;
        }
    });
}

/// The jobs begun here and not finished, by job, with where their records
/// are; one that cannot be read now is not taken for none (NN08).
fn open_jobs(library: &Library) -> Result<BTreeMap<String, PathBuf>, String> {
    match library.checkpoint(OPEN).map_err(|error| error.to_string())? {
        Some(bytes) => serde_json::from_slice(&bytes).map_err(|error| format!("the open jobs: {error}")),
        None => Ok(BTreeMap::new()),
    }
}

/// What the conversations' scopes held here are held as, in one number: it
/// changes as one is held anew.
fn holdings(library: &Library) -> Result<u64, String> {
    use std::hash::{Hash as _, Hasher as _};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for (scope, _, holding) in library.live_scopes().map_err(|error| error.to_string())? {
        if scope.starts_with("c/") || scope.starts_with("b/") {
            (scope, holding).hash(&mut hasher);
        }
    }
    Ok(hasher.finish())
}

/// One look at the jobs running or ended within the day, and those begun
/// here and not finished since (MM21); `whole`, or a scope held anew since
/// `held`, every job since the jobs here are written (NN07, L04). What the
/// scopes are held as now.
fn look(node: &Arc<Node>, whole: bool, held: u64) -> Result<u64, String> {
    let Some(kept) = node.assets.as_ref() else { return Ok(held) };
    let library = &kept.library;
    let error = |error: fastctx_content::Error| error.to_string();
    let written_since = library
        .checkpoint(SINCE)
        .map_err(error)?
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .and_then(|text| text.parse::<jiff::Timestamp>().ok());
    let now_held = holdings(library)?;
    let day = jiff::Timestamp::now() - jiff::SignedDuration::from_hours(24);
    let found = match written_since {
        // Every job since then, past the index's day (M07).
        Some(written) if whole || now_held != held => crate::job_index::ended_since(&node.world, written.min(day)),
        _ => crate::job_index::recent(&node.world, day),
    };
    let mut jobs: BTreeMap<String, (JobRecord, PathBuf)> =
        found.map_err(|error| error.to_string())?.into_iter().map(|(job, path)| (key_of(&job), (job, path))).collect();
    let open = open_jobs(library)?;
    // Those open whose record cannot be read now stay open as they are.
    let mut still = BTreeMap::new();
    for (key, path) in &open {
        if jobs.contains_key(key) {
            continue;
        }
        match fastctx_hands::run::record::read(path) {
            Ok(job) => {
                jobs.entry(key_of(&job)).or_insert((job, path.clone()));
            }
            Err(_) => {
                still.insert(key.clone(), path.clone());
            }
        }
    }
    let mut failed = None;
    for (key, (job, path)) in jobs {
        let Some(home) = home_of(node, &job) else {
            if open.contains_key(&key) {
                still.insert(key, path);
            }
            continue;
        };
        match write_job(node, &home, &job) {
            Ok(true) => {}
            Ok(false) => {
                still.insert(key, path);
            }
            Err(error) => {
                failed.get_or_insert(format!("{}: {error}", job.id));
                still.insert(key, path);
            }
        }
    }
    // A job whose record is gone is open no more; one that cannot be
    // looked at now stays (NN08).
    still.retain(|_, path| !matches!(fastctx_platform::disk::metadata(path), Err(error) if error.kind() == std::io::ErrorKind::NotFound));
    if still != open {
        library.keep_checkpoint(OPEN, serde_json::to_vec(&still).map_err(|error| error.to_string())?).map_err(error)?;
    }
    failed.map_or(Ok(now_held), Err)
}

/// The scope a job's assets go in at `home`, and its holding, when this
/// machine orders their streams there.
fn writable(
    node: &Arc<Node>,
    library: &Library,
    home: &Home,
    job: &JobRecord,
) -> Result<Option<(String, i64)>, String> {
    match home {
        Home::Conversation(resident) => conversation_writable(node, library, resident),
        Home::Account(account) => {
            // An account opened after the job began is another of that
            // name: what ran for the one before goes into no records of
            // this one's, and stays here as it is (`kept_for_good`).
            if opened_of(node, account).is_none() || !ran_for(node, account, job) {
                return Ok(None);
            }
            let (facts, output) = streams_of(node, home);
            let Some((scope, holding)) =
                crate::assets::activity::writable(node, library, &account.hub, &account.account, &facts)?
            else {
                return Ok(None);
            };
            let device = node.assets.as_ref().map(|assets| assets.device()).unwrap_or_default();
            if library
                .orderer(&scope, &output)
                .map_err(|error| error.to_string())?
                .is_some_and(|orderer| orderer != device)
            {
                return Ok(None);
            }
            Ok(Some((scope, holding)))
        }
    }
}

/// The scope `resident`'s jobs go in, when this machine orders its job
/// streams there.
fn conversation_writable(
    node: &Arc<Node>,
    library: &Library,
    resident: &ResidentId,
) -> Result<Option<(String, i64)>, String> {
    let Some((hub, scope)) = crate::timeline::scope_of(node, resident).map_err(|failure| failure.message)? else {
        return Ok(None);
    };
    let Some(kept) = node.assets.as_ref() else { return Ok(None) };
    if crate::assets::release::released_away(node.world.conversation(resident).path(), &scope, &kept.device()) {
        return Ok(None);
    }
    let error = |error: fastctx_content::Error| error.to_string();
    // Begun by the conversation's timeline or words, as theirs is; one from
    // elsewhere not before the hub described it and every stream of it was
    // pulled here once (AG10, MM02).
    let Some(holding) = library.holding(&scope).map_err(error)? else { return Ok(None) };
    let born_elsewhere = node.residents.birth_machine(resident).is_some_and(|born| !node.is_me(&born));
    if born_elsewhere && !(library.is_live(&scope).map_err(error)? && library.is_pulled(&scope).map_err(error)?) {
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

/// How long a part of a job's output is; none when its file is gone, so
/// nothing more of it comes; one that cannot be looked at now is looked at
/// again, never taken as empty (AQ11, MM23, K09).
fn length_of(path: &str) -> Result<Option<u64>, String> {
    match fastctx_platform::disk::metadata(path) {
        Ok(metadata) => Ok(Some(metadata.len())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("looking at {path}: {error}")),
    }
}

/// Writes what is new of `job`, whose assets go at `home`: whether
/// everything of it is written now.
fn write_job(node: &Arc<Node>, home: &Home, job: &JobRecord) -> Result<bool, String> {
    let Some(kept) = node.assets.as_ref() else { return Ok(true) };
    let library = &kept.library;
    let error = |error: fastctx_content::Error| error.to_string();
    let name = checkpoint(job);
    let named = item_of(home, job);
    let Some((scope, holding)) = writable(node, library, home, job)? else { return Ok(false) };
    let (facts_stream, output_stream) = streams_of(node, home);
    let mut standing: Standing = library
        .checkpoint(&name)
        .map_err(error)?
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    if standing.holding != holding {
        standing = Standing { holding, ..Standing::default() };
    }
    if standing.done {
        return Ok(true);
    }
    let signer = kept.signer();
    let mut changed = false;
    // Its facts, as its record stands: a script too large for the record is
    // its one object, whole.
    let mut facts = serde_json::to_value(job).map_err(|error| error.to_string())?;
    let mut script = None;
    if serde_json::to_vec(&facts).map_or(0, |bytes| bytes.len()) > FACTS_BYTES {
        facts["script"] = serde_json::json!({ "in_object": true, "bytes": job.script.len() });
        script = Some(job.script.as_bytes());
    }
    let facts = serde_json::to_vec(&facts).map_err(|error| error.to_string())?;
    let digest: String = Sha256::digest(&facts).iter().map(|byte| format!("{byte:02x}")).collect();
    if digest != standing.facts {
        let item = named.clone().into_bytes();
        let lease = script.map(|mut bytes| library.put(&mut bytes, "job-script", LEASE)).transpose().map_err(error)?;
        let objects = lease.iter().map(|lease| lease.object).collect();
        Writer::<JobFacts>::new(library, &scope, &facts_stream)
            .map_err(error)?
            .in_holding(holding)
            .write_all(
                vec![Write { order: item.clone(), item, value: Some(facts), objects }],
                lease.into_iter().collect(),
                false,
                signer.clone(),
            )
            .map_err(error)?;
        standing.facts = digest;
        changed = true;
    }
    // Its output, what came since.
    let output = Writer::<JobOutput>::new(library, &scope, &output_stream).map_err(error)?.in_holding(holding);
    let mut whole = true;
    // The parts whose file went before all of it was written: the end says
    // so, never that they were written whole (NN09).
    let mut missing = Vec::new();
    for (part, path) in [("output", Some(job.output.as_str())), ("stderr", job.stderr.as_deref())] {
        let Some(path) = path else { continue };
        let at = if part == "output" { &mut standing.output } else { &mut standing.stderr };
        let length = match length_of(path)? {
            Some(length) => length,
            None => {
                // Gone before all it kept was written: so the end says,
                // the output's or the stderr's, as their loss records it, or
                // as much as was written before the file went (OO07).
                let lost = if part == "output" { job.output_lost.as_ref() } else { job.stderr_lost.as_ref() };
                let drained = job
                    .output_drained
                    .as_ref()
                    .map(|drained| if part == "output" { drained.stdout } else { drained.stderr });
                if lost.is_some_and(|lost| lost.kept_bytes > *at)
                    || drained.is_some_and(|drained| drained > *at)
                    || *at > 0
                {
                    missing.push(part);
                }
                *at
            }
        };
        let mut budget = A_PART;
        while *at < length && budget > 0 {
            let item = format!("{named}/{part}/{:016x}", *at).into_bytes();
            // Written before a stop that kept no checkpoint: as it was, on
            // from its end (AQ08, MM19, K07).
            if let Some(entry) = library.item(&scope, &output_stream, &item).map_err(error)? {
                let written = entry.objects.first().map_or(0, |bytes| bytes.size);
                if written == 0 {
                    return Err(format!("the piece of {} at {} is empty", job.id, *at));
                }
                *at += written;
                changed = true;
                continue;
            }
            let take = (length - *at).min(PIECE).min(budget);
            let mut file = fastctx_platform::disk::open(path).map_err(|error| error.to_string())?;
            file.seek(SeekFrom::Start(*at)).map_err(|error| error.to_string())?;
            let mut bytes = Vec::with_capacity(take as usize);
            file.take(take).read_to_end(&mut bytes).map_err(|error| error.to_string())?;
            if bytes.is_empty() {
                break;
            }
            let lease = library.put(&mut bytes.as_slice(), "job-output", LEASE).map_err(error)?;
            let body = serde_json::to_vec(
                // Named as the job is in this stream (`JobOutput::authorize`).
                &serde_json::json!({ "job": named, "part": part, "offset": *at, "length": bytes.len() }),
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
    // Ended, every byte drained — or ended long enough ago that no more
    // comes — and written: its end, once.
    let drained = job.output_ended_at.is_some()
        || job.output_drained.is_some()
        || job.end.as_ref().is_some_and(|end| jiff::Timestamp::now().duration_since(end.at).unsigned_abs() > SETTLED);
    if job.end.is_some() && drained && whole {
        let item = format!("{named}/end").into_bytes();
        if library.item(&scope, &output_stream, &item).map_err(error)?.is_none() {
            let body = serde_json::to_vec(&serde_json::json!({
                "job": named,
                "end": true,
                "output": standing.output,
                "stderr": standing.stderr,
                "missing": missing,
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
        }
        standing.done = true;
        changed = true;
    }
    if changed {
        library
            .keep_checkpoint(&name, serde_json::to_vec(&standing).map_err(|error| error.to_string())?)
            .map_err(error)?;
    }
    Ok(standing.done)
}

/// Whether `job`'s files may go as their retention says: its output is
/// written into the asset library whole and the hub took it — or it keeps
/// no assets here at all, or ended before the jobs here were written at
/// all. Call on a thread that may block.
pub(crate) fn kept_for_good(node: &Node, job: &JobRecord) -> bool {
    let Some(kept) = node.assets.as_ref() else { return true };
    let Some(home) = home_of(node, job) else { return true };
    let library = &kept.library;
    let scope = match &home {
        Home::Conversation(resident) => match crate::timeline::scope_of(node, resident) {
            Ok(Some((_, scope))) => Some(scope),
            _ => return true,
        },
        // An account not known here now may be one this machine no longer
        // sees, not one gone, and one opened after the job began another of
        // its name: what was written of the job cannot be told taken by the
        // hub, so its files stay.
        Home::Account(account) => opened_of(node, account).filter(|_| ran_for(node, account, job)).map(|opened| {
            fastctx_types::assets::records_key(account.hub.as_str(), &account.account, opened.as_millisecond())
        }),
    };
    let (facts_stream, output_stream) = streams_of(node, &home);
    let standing = library
        .checkpoint(&checkpoint(job))
        .ok()
        .flatten()
        .and_then(|bytes| serde_json::from_slice::<Standing>(&bytes).ok());
    match standing {
        Some(standing) if standing.done => {}
        Some(_) => return false,
        // Never begun: one that ended before the jobs here were written at
        // all — of before this release — goes as it always did; any other
        // waits to be written (K08).
        None => {
            let since = library
                .checkpoint(SINCE)
                .ok()
                .flatten()
                .and_then(|bytes| String::from_utf8(bytes).ok())
                .and_then(|text| text.parse::<jiff::Timestamp>().ok());
            return match (since, job.end.as_ref()) {
                (Some(since), Some(end)) => end.at < since,
                _ => false,
            };
        }
    }
    let Some(scope) = scope else { return false };
    // Every frame of the job streams here went up.
    library
        .uploads(&scope, &kept.device())
        .is_ok_and(|uploads| uploads.iter().all(|(stream, _)| *stream != output_stream && *stream != facts_stream))
}
