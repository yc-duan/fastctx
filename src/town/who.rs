//! What one machine knows about the residents and running things on it: the answer each
//! machine gives when someone runs `fastctx w` (`specs/world-town.md`). Everything here is read
//! from the records the town writes; nothing comes from memory, so the node and a plain
//! `fastctx w` read the same facts.

use super::records::{About, MessageLine, Now, Records};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The actions shown for one resident in a detail view: twenty lines are about a thousand
/// characters, enough to see what it has been doing lately; the head names the file with all.
pub(crate) const DETAIL_ACTIONS: usize = 20;

/// What `fastctx w` asks every machine.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(crate) struct Query {
    /// The town to look at; `None` asks for the World level: every town and the outside.
    #[serde(default)]
    pub(crate) town: Option<String>,
    /// Also residents that are asleep or have left.
    #[serde(default)]
    pub(crate) all: bool,
    /// Particular residents or jobs, for their details; when set, `town` and `all` are ignored.
    #[serde(default)]
    pub(crate) ids: Vec<String>,
}

/// One machine's answer.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(crate) struct MachineAnswer {
    pub(crate) machine: String,
    pub(crate) residents: Vec<ResidentFacts>,
    pub(crate) jobs: Vec<JobFacts>,
    /// At the World level, each town with residents on this machine.
    #[serde(default)]
    pub(crate) towns: Vec<TownCount>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct ResidentFacts {
    pub(crate) about: About,
    pub(crate) now: Now,
    /// Its records directory on this machine.
    pub(crate) dir: PathBuf,
    /// Detail only: the most recent actions, oldest first, and how many there are in all.
    #[serde(default)]
    pub(crate) actions: Vec<ActionLine>,
    #[serde(default)]
    pub(crate) actions_total: usize,
    /// Detail only: requests, words spoken to it, and its answers.
    #[serde(default)]
    pub(crate) messages: Vec<MessageLine>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct ActionLine {
    pub(crate) at: String,
    pub(crate) machine: String,
    pub(crate) hand: String,
    pub(crate) what: String,
    pub(crate) result: String,
}

/// A command a resident started on this machine, as its job record says.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct JobFacts {
    /// Who started it, from the directory it is recorded under.
    pub(crate) resident: String,
    pub(crate) town: Option<String>,
    pub(crate) id: String,
    #[serde(default)]
    pub(crate) pid: Option<u32>,
    pub(crate) command: String,
    pub(crate) machine: String,
    pub(crate) started_at: String,
    #[serde(default)]
    pub(crate) ended_at: Option<String>,
    pub(crate) state: String,
    #[serde(default)]
    pub(crate) exit_code: Option<i32>,
    pub(crate) output_path: PathBuf,
}

impl JobFacts {
    pub(crate) fn running(&self) -> bool {
        matches!(self.state.as_str(), "starting" | "running")
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct TownCount {
    pub(crate) town: String,
    pub(crate) here: usize,
    pub(crate) busy: usize,
}

/// Whether a resident in this state counts as here; the rest show only with `-a`.
pub(crate) fn is_here(state: &str) -> bool {
    matches!(state, "busy" | "idle" | "waiting" | "unknown")
}

/// Answers a query from this machine's records under `world_dir`.
pub(crate) fn answer(world_dir: &Path, machine: &str, query: &Query) -> MachineAnswer {
    let mut answer = MachineAnswer { machine: machine.to_string(), ..MachineAnswer::default() };
    let places = places(world_dir);
    if !query.ids.is_empty() {
        for (town, residents_dir) in &places {
            for dir in subdirectories(residents_dir) {
                let id = file_name(&dir);
                if query.ids.contains(&id) {
                    if let Some(facts) = resident(&dir, true) {
                        answer.residents.push(facts);
                    }
                }
                answer.jobs.extend(jobs(&dir, &id, town.as_deref()).into_iter().filter(|job| query.ids.contains(&job.id)));
            }
        }
        return answer;
    }
    for (town, residents_dir) in &places {
        let wanted = match (&query.town, town) {
            (Some(asked), Some(town)) => asked == town,
            (Some(_), None) => false,
            (None, None) => true,
            // At the World level a town shows as a count, its residents stay inside it.
            (None, Some(_)) => false,
        };
        let mut count = TownCount { town: town.clone().unwrap_or_default(), here: 0, busy: 0 };
        for dir in subdirectories(residents_dir) {
            let id = file_name(&dir);
            let running: Vec<JobFacts> = jobs(&dir, &id, town.as_deref()).into_iter().filter(JobFacts::running).collect();
            let facts = resident(&dir, false);
            if let Some(facts) = &facts
                && is_here(&facts.now.state)
            {
                count.here += 1;
                count.busy += usize::from(facts.now.state == "busy");
            }
            if !wanted {
                continue;
            }
            // A resident that left still shows while something it started runs on: the line
            // under `unattended` says who started it and when they left.
            if let Some(facts) = facts
                && (query.all || is_here(&facts.now.state) || !running.is_empty())
            {
                answer.residents.push(facts);
            }
            answer.jobs.extend(running);
        }
        if query.town.is_none()
            && town.is_some()
            && (count.here > 0 || query.all)
        {
            answer.towns.push(count);
        }
    }
    answer
}

/// Every place residents are recorded on this machine: each town, and the outside.
fn places(world_dir: &Path) -> Vec<(Option<String>, PathBuf)> {
    let mut places = vec![(None, world_dir.join("outside").join("residents"))];
    for town in subdirectories(&world_dir.join("towns")) {
        places.push((Some(file_name(&town)), town.join("residents")));
    }
    places
}

/// A resident's facts; `None` for a directory that only holds jobs of a resident living on
/// another machine.
fn resident(dir: &Path, detail: bool) -> Option<ResidentFacts> {
    let records = Records::new(dir.to_path_buf());
    let about = records.about().ok().flatten()?;
    let now = records.now().ok().flatten().unwrap_or_default();
    let mut facts = ResidentFacts { about, now, dir: dir.to_path_buf(), actions: Vec::new(), actions_total: 0, messages: Vec::new() };
    if detail {
        if let Ok((lines, total)) = records.recent_actions(DETAIL_ACTIONS) {
            facts.actions = lines.iter().filter_map(|line| parse_action(line)).collect();
            facts.actions_total = total;
        }
        facts.messages = records.messages().unwrap_or_default();
    }
    Some(facts)
}

fn parse_action(line: &str) -> Option<ActionLine> {
    let mut fields = line.splitn(5, '\t');
    Some(ActionLine {
        at: fields.next()?.to_string(),
        machine: fields.next()?.to_string(),
        hand: fields.next()?.to_string(),
        what: fields.next()?.to_string(),
        result: fields.next().unwrap_or_default().to_string(),
    })
}

/// The job records under a resident's directory, as the run engine writes them.
fn jobs(dir: &Path, resident: &str, town: Option<&str>) -> Vec<JobFacts> {
    #[derive(Deserialize)]
    struct Record {
        id: String,
        #[serde(default)]
        pid: Option<u32>,
        command: String,
        machine: String,
        started_at: String,
        #[serde(default)]
        ended_at: Option<String>,
        state: String,
        #[serde(default)]
        exit_code: Option<i32>,
        output_path: PathBuf,
    }
    std::fs::read_dir(dir.join("jobs"))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.path().extension().is_some_and(|extension| extension == "json"))
        .filter_map(|entry| serde_json::from_slice::<Record>(&std::fs::read(entry.path()).ok()?).ok())
        .map(|record| JobFacts {
            resident: resident.to_string(),
            town: town.map(str::to_string),
            id: record.id,
            pid: record.pid,
            command: record.command,
            machine: record.machine,
            started_at: record.started_at,
            ended_at: record.ended_at,
            state: record.state,
            exit_code: record.exit_code,
            output_path: record.output_path,
        })
        .collect()
}

fn subdirectories(dir: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    found.sort();
    found
}

fn file_name(path: &Path) -> String {
    path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default()
}
