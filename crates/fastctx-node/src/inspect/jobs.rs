//! The account's jobs on this machine and their output, as a look shows
//! them: those running, and those that ended within the last day.

use std::path::{Path, PathBuf};

use fastctx_types::access::HubAccount;
use fastctx_types::inspect::{Inspected, MOST_BYTES};
use fastctx_types::ipc::Failure;
use fastctx_types::run::{EndedHow, JobOwner, JobRecord};

use crate::Node;

/// How long a job that ended is still listed.
const LISTED_FOR: jiff::SignedDuration = jiff::SignedDuration::from_hours(24);
/// The most jobs a look lists.
const LISTED: usize = 50;
/// How much of a job's command a listing shows.
const COMMAND: usize = 120;

/// The account's jobs here, the latest started first.
pub(super) fn list(node: &Node, scope: &HubAccount) -> Result<Inspected, Failure> {
    let since = jiff::Timestamp::now() - LISTED_FOR;
    let mut jobs: Vec<JobRecord> = records(node)?
        .into_iter()
        .map(|(record, _)| record)
        .filter(|record| of(node, record, scope) && record.end.as_ref().is_none_or(|end| end.at >= since))
        .collect();
    if jobs.is_empty() {
        return Ok(super::noted("no job of this account here has run within the last day".to_string()));
    }
    jobs.sort_by(|a, b| b.started_at.cmp(&a.started_at));
    let mut text = String::new();
    for job in jobs.iter().take(LISTED) {
        let command = job.script.lines().find(|line| !line.trim().is_empty()).unwrap_or_default().trim();
        let command = super::machine::within(&super::redact::command(command), COMMAND);
        let owner = job.owner.resident().map(|owner| format!("  for {}", owner.resident)).unwrap_or_default();
        text.push_str(&format!(
            "{}  {}  started {}{owner}  {command}\n",
            job.id,
            state(job),
            super::machine::moment(job.started_at)
        ));
    }
    let note = (jobs.len() > LISTED).then(|| format!("{} jobs; the {LISTED} started last are shown", jobs.len()));
    Ok(Inspected { text, note })
}

/// The output of the account's job `job` here, as `tail` or `matching` ask:
/// what it wrote, then what it wrote to its errors where they were kept
/// apart, each given half of what a look shows when both are there.
pub(super) fn output(
    node: &Node,
    scope: &HubAccount,
    job: &str,
    tail: Option<u32>,
    matching: Option<&str>,
) -> Result<Inspected, Failure> {
    let Some((record, _)) = records(node)?.into_iter().find(|(record, _)| record.id.as_str() == job) else {
        return Ok(super::noted(format!("no job {job} here")));
    };
    if !of(node, &record, scope) {
        return Err(Failure::not_run(format!("{job} is not a job of this account")));
    }
    let mut notes = vec![format!("{job} {}", state(&record))];
    let mut read = |path: Option<&String>, what: &str| -> Result<Option<String>, Failure> {
        let Some(path) = path.map(PathBuf::from) else { return Ok(None) };
        if !fastctx_platform::disk::is_file(&path) {
            notes.push(format!("it kept no {what}"));
            return Ok(None);
        }
        let seen = super::files::read_at(&path, &format!("{job}'s {what}"), tail, matching)?;
        notes.extend(seen.note);
        Ok(Some(seen.text))
    };
    let output = read(Some(&record.output), "output")?.unwrap_or_default();
    let errors = read(record.stderr.as_ref(), "errors")?.filter(|errors| !errors.is_empty());
    let text = match errors {
        None => output,
        Some(errors) => {
            // Each within half of what a look shows, the line break it may
            // gain counted.
            let half = MOST_BYTES / 2 - 1;
            let (output, errors) = (last_of(&output, half), last_of(&errors, half));
            let first = output.lines().count() + 1;
            notes.push(format!("from line {first} on, what it wrote to its errors"));
            format!("{output}{errors}")
        }
    };
    Ok(Inspected { text, note: Some(notes.join("; ")) })
}

/// The last lines of `text` within `most` bytes, whole where there is more
/// than one, ending in a line break.
fn last_of(text: &str, most: usize) -> String {
    let mut start = text.len().saturating_sub(most);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    if start > 0
        && let Some(at) = text[start..].find('\n')
        && start + at + 1 < text.len()
    {
        start += at + 1;
    }
    let kept = &text[start..];
    if kept.is_empty() || kept.ends_with('\n') { kept.to_string() } else { format!("{kept}\n") }
}

/// Whether `record` is a job of the account `scope`: sent here for it, or
/// run by a conversation or a page here that works for it.
fn of(node: &Node, record: &JobRecord, scope: &HubAccount) -> bool {
    if let Some(account) = record.on.as_ref().and_then(|on| on.account.as_ref()).or(record.account.as_ref()) {
        return account == scope;
    }
    let acting = match &record.owner {
        JobOwner::Resident(address) if node.is_me(&address.machine) => {
            node.residents.about(&address.resident).map(|(about, _)| about.acting)
        }
        JobOwner::Resident(_) => None,
        JobOwner::Page { page } => crate::pages::identity::acting(node, page).ok(),
    };
    acting.is_some_and(|acting| node.serves(&acting, &scope.hub, &scope.account))
}

/// How a job stands, in a few words.
fn state(record: &JobRecord) -> String {
    match &record.end {
        None => "running".to_string(),
        Some(end) => {
            let at = super::machine::moment(end.at);
            match &end.how {
                EndedHow::Finished => match &record.exit {
                    Some(exited) => format!("ended {at}, {}", exited.status),
                    None => format!("ended {at}"),
                },
                EndedHow::Stopped => format!("stopped {at}"),
                EndedHow::Interrupted { reason } => format!("interrupted {at}: {reason}"),
                EndedHow::Failed { error } => format!("failed {at}: {error}"),
            }
        }
    }
}

/// Every job record here, with where it is kept.
fn records(node: &Node) -> Result<Vec<(JobRecord, PathBuf)>, Failure> {
    let failed = |error: std::io::Error| Failure::new(format!("reading the jobs here: {error}"));
    let mut found = Vec::new();
    for parent in node.world.job_parents().map_err(failed)? {
        for owner in entries(&parent).map_err(failed)? {
            for path in entries(&owner.join("jobs")).map_err(failed)? {
                if path.extension().is_none_or(|extension| extension != "json") {
                    continue;
                }
                match fastctx_hands::run::record::read(&path) {
                    Ok(record) => found.push((record, path)),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(failed(error)),
                }
            }
        }
    }
    Ok(found)
}

/// What a directory holds; nothing when it is not there.
fn entries(path: &Path) -> std::io::Result<Vec<PathBuf>> {
    match fastctx_platform::disk::read_dir(path) {
        Ok(entries) => entries.map(|entry| entry.map(|entry| entry.path())).collect(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error),
    }
}
