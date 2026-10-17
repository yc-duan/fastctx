//! Facts recovered from existing records when someone returns to the World.
//!
//! Times are the timestamps actually retained by each source, not a reconstructed
//! history. Nodes do not share a clock. A query reads live files, not an atomic
//! distributed snapshot, and does not create a cursor, cache, or event store.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::project::{Location, Writer};
use crate::residents::{Host, Origin, Outcome};
use crate::run::{EndedHow, Exited};
use crate::session::TurnEnd;
use crate::{Address, EntryId, JobId, JournalName, MachineName, NoticeId, TownName};

/// The complete answer, with one chronologically sorted, deduplicated timeline.
/// `None` means the entire World, independently of the caller's current town.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorldSince {
    pub machine: MachineName,
    /// Exclusive lower bound on the recorded timestamps in `events`.
    pub since: Timestamp,
    pub town: Option<TownName>,
    pub machines: Vec<MachineSince>,
    pub events: Vec<SinceEvent>,
    /// Surviving facts with no event time. These cannot be filtered by `since`
    /// and must never be presented as events during the user's absence.
    pub undated: Vec<UndatedFact>,
    /// Failures while choosing machines or reconciling their answers.
    pub issues: Vec<SinceIssue>,
    /// Known limits of the retained history, distinct from failures to read it.
    pub limitations: Vec<String>,
}

/// The same three outcomes as `MachineWho`. Answer metadata stays here; the
/// returned facts are merged into `WorldSince.events` and `WorldSince.undated`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum MachineSince {
    Answered { reply: SinceCoverage },
    Offline { machine: MachineName, since: Option<Timestamp> },
    NoAnswer { machine: MachineName, reason: String },
}

/// One machine's telephone answer, before the querying node merges the facts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SinceReply {
    #[serde(flatten)]
    pub coverage: SinceCoverage,
    pub events: Vec<SinceEvent>,
    pub undated: Vec<UndatedFact>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SinceCoverage {
    pub machine: MachineName,
    pub read_started_at: Timestamp,
    pub read_finished_at: Timestamp,
    /// A machine can answer while some records are unreadable. Every such
    /// omission is explicit here; an empty timeline is not proof of inactivity.
    pub issues: Vec<SinceIssue>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SinceIssue {
    pub source: String,
    pub reason: String,
}

/// Exact town evidence, or the directories holding a copied record. `None`
/// inside either variant means outside all towns. Copied turn/birth records
/// do not encode their original town; a town query includes these uncertain
/// matches explicitly instead of attributing them to their current directory.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum SinceTown {
    Known { town: Option<TownName> },
    Unrecorded { recorded_in: Vec<Option<TownName>> },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SinceEvent {
    /// Stable identity across repeated queries and replicated ledger copies.
    pub id: String,
    /// Timestamp of the fact as defined by its kind below; never file mtime.
    pub at: Timestamp,
    pub machine: MachineName,
    pub town: SinceTown,
    /// Existing records supporting this fact, on the machines that answered.
    pub sources: Vec<Location>,
    #[serde(flatten)]
    pub what: SinceKind,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum SinceKind {
    /// A World write action, including its exact recorded result. The action
    /// timestamp is when the call began, not a filesystem modification time.
    FileAction {
        resident: Address,
        hand: String,
        paths: String,
        result: String,
    },
    /// First resident creation, not every reconnection. `at` is `born_at`.
    ResidentBorn {
        resident: Address,
        host: Host,
        origin: Origin,
    },
    /// `at` is the host's reported end when retained, otherwise the node's
    /// first settlement, including discovery after a restart.
    /// `end` preserves the retained outcome, reason, error and last message.
    /// A colleague's answer is that last message; this is not a delivery receipt.
    /// An empty turn id means no host turn id was ever known. `None` readers
    /// means an older session event did not record who was owed this turn.
    /// Hosts conflate user stop with interruption; no extra cause is inferred.
    TurnEnded {
        resident: Address,
        turn: String,
        end: TurnEnd,
        readers: Option<Vec<Address>>,
    },
    /// `at` is the supervisor's final account, or discovery of its loss.
    /// `elapsed_ms` measures from the job's launch record to that account,
    /// including launch overhead; it is NOT an actual runtime for Interrupted
    /// or Failed. Shell exit has its own timestamp, preserved in `exit`.
    JobEnded {
        job: JobId,
        owner: crate::run::JobOwner,
        started_at: Timestamp,
        elapsed_ms: i64,
        how: EndedHow,
        exit: Option<Exited>,
        script: String,
        output: String,
        /// The account a job run here for a resident of another machine
        /// works for, by which it is seen.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        account: Option<crate::access::HubAccount>,
    },
    /// A successful invitation. `at` is when the caller started asking, not
    /// when the colleague was born or when the call returned.
    ColleagueAsked {
        asker: Address,
        colleague: Address,
        label: String,
    },
    /// A recorded attempt to return a turn's answer. Sent and Waiting do not
    /// mean the recipient read it; preserve the recorded outcome verbatim.
    AnswerDelivery {
        colleague: Address,
        to: Address,
        turn: Option<String>,
        outcome: Outcome,
    },
    /// Only surviving departure markers exist; returning deletes one and a
    /// later departure replaces it. `at` is the surviving marker's timestamp.
    ResidentMoved {
        resident: Address,
        from: Option<TownName>,
        to: Option<TownName>,
    },
    /// Latest retained mod activation. `at` preserves its logical timestamp:
    /// milliseconds plus a submillisecond ordering suffix. The suffix orders
    /// records; it is not a submillisecond clock measurement.
    ClaudeRegistered {
        resident: Address,
        session: String,
        instance: Option<String>,
    },
    /// The node recorded that submission acceptance could not be established.
    /// This does not claim that a host turn existed or actually ended at `at`.
    SubmissionUncertain {
        resident: Address,
        delivery: NoticeId,
        why: String,
        end: Option<TurnEnd>,
        readers: Option<Vec<Address>>,
    },
    JournalEntry {
        journal: JournalName,
        entry: EntryId,
        by: Writer,
    },
    JournalDescription {
        journal: JournalName,
        version: u32,
        by: Writer,
    },
    ProjectVersion {
        version: u32,
        name: String,
        by: Writer,
    },
    /// The hub says the current online interval began at `at`. This does not
    /// assert any earlier disconnect or the number of intervening changes.
    MachineConnected,
    /// The hub last observed this machine leave at `at`; no earlier intervals
    /// are retained. A hub restart may leave the time unknown instead.
    MachineOffline,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UndatedFact {
    pub resident: Address,
    pub town: SinceTown,
    pub source: Location,
    #[serde(flatten)]
    pub what: UndatedKind,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum UndatedKind {
    /// Current retained state. `now.since` is not consistently a sleep time.
    Asleep,
    /// The marker records closure, with no time or reason.
    ClaudeClosed { session: String, instance: String },
    /// Older registrations did not carry a timestamp.
    ClaudeRegistered { session: String, instance: Option<String> },
    /// A durable unresolved handover can precede its timestamped session event.
    SubmissionUncertain { delivery: NoticeId, why: String, end: Option<TurnEnd>, readers: Option<Vec<Address>> },
}

impl MachineSince {
    /// The machine the outcome is of.
    pub fn machine(&self) -> &MachineName {
        match self {
            MachineSince::Answered { reply } => &reply.machine,
            MachineSince::Offline { machine, .. } | MachineSince::NoAnswer { machine, .. } => machine,
        }
    }
}

/// Merges what several machines answered: the same fact from several
/// copies is one event with every source; copies that disagree are both
/// kept, and the disagreement is an issue.
pub fn merge_events(events: Vec<SinceEvent>, issues: &mut Vec<SinceIssue>) -> Vec<SinceEvent> {
    let mut unique = std::collections::BTreeMap::<String, SinceEvent>::new();
    let mut conflicts = Vec::new();
    for mut event in events {
        match unique.get_mut(&event.id) {
            None => {
                unique.insert(event.id.clone(), event);
            }
            Some(prior) if prior.at == event.at && prior.machine == event.machine && prior.what == event.what => {
                for source in event.sources.drain(..) {
                    if !prior.sources.contains(&source) {
                        prior.sources.push(source);
                    }
                }
                merge_town(&mut prior.town, &event.town);
            }
            Some(_) => {
                issues.push(SinceIssue {
                    source: event.id.clone(),
                    reason: "copies disagree; both recorded facts are returned".to_string(),
                });
                conflicts.push(event);
            }
        }
    }
    let mut events: Vec<_> = unique.into_values().chain(conflicts).collect();
    events.sort_by(|a, b| (a.at, &a.id).cmp(&(b.at, &b.id)));
    events
}

/// The known limits of the retained history.
pub fn limitations() -> Vec<String> {
    [
        "Resident births are retained, but repeated arrivals and departures have no complete history. Current sleep and Claude closure markers have no reliable event time and are returned separately.",
        "Returning to a town deletes its departure marker; leaving it again replaces the marker. Only surviving moves can be reported. Copied births and turns do not record their original town; uncertain town matches are marked unrecorded.",
        "Turn timestamps use the host-reported end when retained, otherwise the node's first settlement. Host stop and interruption share an outcome; only a retained reason can distinguish them. Sessions whose turns the World never observes have no turn history.",
        "Job elapsed time includes launch overhead. An interrupted or failed supervisor's final account does not establish when, or whether, its processes stopped.",
        "The hub supplies only the current connection endpoint: connected since, or last observed offline. Earlier intervals are unavailable; after a hub restart an offline time can be unknown.",
        "Each machine reads its existing records and ledger copies at query time. Clock readings are preserved without cross-machine correction; this is not a distributed snapshot. Timestamps retain their source precision, not a promise of nanosecond accuracy.",
    ].into_iter().map(str::to_string).collect()
}

/// Takes what `other` knows of where an event happened into `prior`.
pub fn merge_town(prior: &mut SinceTown, other: &SinceTown) {
    match (&mut *prior, other) {
        (SinceTown::Unrecorded { recorded_in }, SinceTown::Unrecorded { recorded_in: more }) => {
            recorded_in.extend_from_slice(more);
            recorded_in.sort();
            recorded_in.dedup();
        }
        (SinceTown::Unrecorded { .. }, SinceTown::Known { .. }) => *prior = other.clone(),
        _ => {}
    }
}
