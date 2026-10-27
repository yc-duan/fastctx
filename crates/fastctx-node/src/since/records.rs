//! Read existing records. Each conversation keeps them in one directory; the
//! project a fact belongs to is the one the conversation was in when it
//! happened, from its `town` record. Turn records can be written more than
//! once, by different paths, and are merged by identity.

use std::collections::BTreeMap;
use std::io::{self, BufRead};
use std::path::{Path, PathBuf};

use fastctx_net::ledger::Entry;
use fastctx_net::sign::Signed;
use fastctx_types::layout::{ConversationDir, Kind, WorldDir};
use fastctx_types::project::{Location, Record, version_of};
use fastctx_types::residents::{About, Action, Activity, MessageKind, MessageLine, Places};
use fastctx_types::run::JobRecord;
use fastctx_types::session::{Happened, SessionEvent, TurnEnd};
use fastctx_types::since::*;
use fastctx_types::{Address, MachineName, NoticeId, ResidentId, TownName};
use jiff::Timestamp;

use crate::delivery::Finished;

pub(super) fn identity(kind: &str, parts: &[&str]) -> String {
    crate::speech::fixed_id(kind, parts).to_string()
}

fn known(town: Option<&TownName>) -> SinceTown {
    SinceTown::Known { town: town.cloned() }
}

fn matches_town(place: &SinceTown, filter: Option<&TownName>) -> bool {
    filter.is_none_or(|filter| match place {
        SinceTown::Known { town } => town.as_ref() == Some(filter),
        SinceTown::Unrecorded { recorded_in } => recorded_in.iter().any(|town| town.as_ref() == Some(filter)),
    })
}

pub(super) fn read(
    world: &WorldDir,
    machine: &MachineName,
    since: Timestamp,
    town: Option<&TownName>,
) -> io::Result<SinceReply> {
    let started = Timestamp::now();
    let mut scan = Scan { machine, events: Vec::new(), undated: Vec::new(), issues: Vec::new() };
    let mut residents = BTreeMap::<ResidentId, ResidentFiles>::new();
    for parent in world.job_parents()? {
        for directory in scan.dirs(&parent) {
            scan.jobs(&directory.join("jobs"));
        }
    }
    let kept = match world.kept() {
        Ok(kept) => kept,
        Err(error) => {
            scan.issue(&world.conversations(), error);
            Vec::new()
        }
    };
    for kept in kept {
        let Kind::Home(_) = &kept.kind else { continue };
        let directory = &kept.directory;
        let Some(about) = scan.optional::<About>(&directory.about()) else { continue };
        let places = match crate::residents::read_places(directory, &about) {
            Ok(places) => places,
            Err(error) => {
                scan.issue(&directory.town(), error);
                continue;
            }
        };
        let removed = match crate::residents::read_removed(directory) {
            Ok(removed) => removed.is_some(),
            Err(error) => {
                scan.issue(&directory.removed(), error);
                false
            }
        };
        let address = Address { machine: machine.clone(), resident: about.id.clone() };
        for (from, moved) in places.departures() {
            scan.events.push(SinceEvent {
                hub: None,
                source: None,
                id: identity("moved", &[&address.key(), &directory.path().to_string_lossy(), &moved.at.to_string()]),
                at: moved.at,
                machine: machine.clone(),
                town: known(from.as_ref()),
                sources: vec![scan.location(&directory.town())],
                what: SinceKind::ResidentMoved { resident: address.clone(), from, to: moved.to },
            });
        }
        if !removed {
            match crate::residents::read_now(directory) {
                Ok(Some(now)) if now.activity == Activity::Asleep => scan.undated.push(UndatedFact {
                    hub: None,
                    resident: address.clone(),
                    town: known(places.now()),
                    source: scan.location(&directory.now()),
                    what: UndatedKind::Asleep,
                }),
                Ok(_) => {}
                Err(error) => scan.issue(&directory.now(), error),
            }
        }
        let files = residents.entry(about.id.clone()).or_default();
        files.birth_sources.push(scan.location(&directory.about()));
        files.about = Some(about.clone());
        files.places = Some(places);
        scan.resident(&about, directory, files);
    }
    for (id, files) in &residents {
        scan.finish_resident(id, files);
    }
    scan.claude(world, &residents);
    let towns = match town {
        Some(town) => vec![town.clone()],
        None => world.towns()?,
    };
    for town in towns {
        scan.ledger(world, &town);
    }
    scan.events.retain(|event| {
        event.at > since
            && (matches_town(&event.town, town)
                || matches!(&event.what, SinceKind::ResidentMoved { to, .. } if town.is_some() && to.as_ref() == town))
    });
    scan.undated.retain(|fact| matches_town(&fact.town, town));
    let events = fastctx_types::since::merge_events(scan.events, &mut scan.issues);
    Ok(SinceReply {
        coverage: SinceCoverage {
            machine: machine.clone(),
            read_started_at: started,
            read_finished_at: Timestamp::now(),
            issues: scan.issues,
        },
        events,
        undated: scan.undated,
    })
}

/// Reads the queried ledger without claiming to know the residents of a
/// town whose machines were not this machine.
pub(super) fn ledger(
    world: &WorldDir,
    machine: &MachineName,
    since: Timestamp,
    town: &TownName,
) -> (Vec<SinceEvent>, Vec<SinceIssue>) {
    let mut scan = Scan { machine, events: Vec::new(), undated: Vec::new(), issues: Vec::new() };
    scan.ledger(world, town);
    scan.events.retain(|event| event.at > since);
    (scan.events, scan.issues)
}

#[derive(Default)]
struct ResidentFiles {
    about: Option<About>,
    places: Option<Places>,
    birth_sources: Vec<Location>,
    turns: BTreeMap<String, Turn>,
    uncertain: BTreeMap<NoticeId, Uncertain>,
}

impl ResidentFiles {
    /// The project the conversation was in at `at`.
    fn town_at(&self, at: Timestamp) -> SinceTown {
        known(self.places.as_ref().and_then(|places| places.at(at)))
    }

    /// The project it is in now, for what has no time of its own.
    fn town_now(&self) -> SinceTown {
        known(self.places.as_ref().and_then(Places::now))
    }
}

#[derive(Default)]
struct Uncertain {
    at: Option<Timestamp>,
    why: String,
    end: Option<TurnEnd>,
    readers: Option<Vec<Address>>,
    sources: Vec<Location>,
}

struct Turn {
    at: Timestamp,
    end: TurnEnd,
    readers: Option<Vec<Address>>,
    /// Terminal records precede message rows, which precede legacy events.
    rank: u8,
    sources: Vec<Location>,
}

struct Scan<'a> {
    machine: &'a MachineName,
    events: Vec<SinceEvent>,
    undated: Vec<UndatedFact>,
    issues: Vec<SinceIssue>,
}

impl Scan<'_> {
    fn location(&self, path: &Path) -> Location {
        Location { machine: self.machine.clone(), path: path.to_string_lossy().into_owned() }
    }

    fn issue(&mut self, path: &Path, error: impl std::fmt::Display) {
        self.issues.push(SinceIssue { source: path.to_string_lossy().into_owned(), reason: error.to_string() });
    }

    fn optional<T: serde::de::DeserializeOwned>(&mut self, path: &Path) -> Option<T> {
        match read_json(path) {
            Ok(value) => Some(value),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => {
                self.issue(path, error);
                None
            }
        }
    }

    fn files(&mut self, directory: &Path) -> Vec<PathBuf> {
        match paths(directory) {
            Ok(paths) => paths,
            Err(error) => {
                self.issue(directory, error);
                Vec::new()
            }
        }
    }

    fn dirs(&mut self, directory: &Path) -> Vec<PathBuf> {
        self.files(directory).into_iter().filter(|path| path.is_dir()).collect()
    }

    fn json_files(&mut self, directory: &Path) -> Vec<PathBuf> {
        self.files(directory)
            .into_iter()
            .filter(|path| {
                path.extension().is_some_and(|ext| ext == "json")
                    && !path.file_name().unwrap().to_string_lossy().starts_with('.')
            })
            .collect()
    }

    fn jobs(&mut self, directory: &Path) {
        for path in self.json_files(directory) {
            let Some(job) = self.optional::<JobRecord>(&path) else { continue };
            let Some(end) = job.end else { continue };
            let account = job.account.or_else(|| job.on.and_then(|on| on.account));
            self.events.push(SinceEvent {
                hub: None,
                source: None,
                id: identity("job-end", &[job.machine.key(), &job.owner.key(), job.id.as_str()]),
                at: end.at,
                machine: job.machine,
                town: known(job.town.as_ref()),
                sources: vec![self.location(&path)],
                what: SinceKind::JobEnded {
                    job: job.id,
                    owner: job.owner,
                    started_at: job.started_at,
                    elapsed_ms: i64::try_from((end.at.as_nanosecond() - job.started_at.as_nanosecond()) / 1_000_000)
                        .expect("the Timestamp range spans fewer than i64::MAX milliseconds"),
                    how: end.how,
                    exit: job.exit,
                    script: job.script,
                    output: job.output,
                    account,
                },
            });
        }
    }

    fn resident(&mut self, about: &About, directory: &ConversationDir, files: &mut ResidentFiles) {
        let address = Address { machine: self.machine.clone(), resident: about.id.clone() };
        for path in self.json_files(&directory.turns()) {
            if let Some(finished) = self.optional::<Finished>(&path) {
                self.turn(
                    files,
                    finished.turn,
                    Turn {
                        at: finished.ended_at.unwrap_or(finished.at),
                        end: finished.end,
                        readers: Some(finished.readers),
                        rank: 3,
                        sources: vec![self.location(&path)],
                    },
                );
            }
        }
        for path in self.json_files(&directory.session_events()) {
            if let Some(event) = self.optional::<SessionEvent>(&path) {
                match event.happened {
                    Happened::TurnEnded { turn, end } => self.turn(
                        files,
                        turn,
                        Turn { at: event.at, end, readers: None, rank: 1, sources: vec![self.location(&path)] },
                    ),
                    Happened::DeliveryUncertain { delivery, why } => {
                        let uncertain = files.uncertain.entry(delivery).or_default();
                        uncertain.at = Some(event.at);
                        uncertain.why = why;
                        uncertain.sources.push(self.location(&path));
                    }
                    _ => {}
                }
            }
        }
        for path in self.json_files(&directory.handovers()) {
            if let Some(handover) = self.optional::<crate::delivery::Handover>(&path)
                && let Some(end) = handover.unresolved
            {
                for input in handover.inputs {
                    let uncertain = files.uncertain.entry(input.delivery.id).or_default();
                    uncertain.why = end.reason.clone().unwrap_or_default();
                    uncertain.end = Some(end.clone());
                    let readers = uncertain.readers.get_or_insert_default();
                    readers.extend(input.readers);
                    readers.sort();
                    readers.dedup();
                    uncertain.sources.push(self.location(&path));
                }
            }
        }
        let path = directory.messages();
        self.lines::<MessageLine>(&path, |scan, line, number| match line.what {
            MessageKind::Ended { turn, end, owed } => scan.turn(
                files,
                turn,
                Turn { at: line.at, end, readers: Some(owed), rank: 2, sources: vec![scan.location(&path)] },
            ),
            MessageKind::Told { to, turn, outcome } => scan.events.push(SinceEvent {
                hub: None,
                source: None,
                id: identity("answer-delivery", &[&address.key(), &path.to_string_lossy(), &number.to_string()]),
                at: line.at,
                machine: scan.machine.clone(),
                town: files.town_at(line.at),
                sources: vec![scan.location(&path)],
                what: SinceKind::AnswerDelivery { colleague: address.clone(), to, turn, outcome },
            }),
            MessageKind::Asked { .. } | MessageKind::Heard { .. } => {}
        });
        let path = directory.actions();
        self.lines::<Action>(&path, |scan, action, number| {
            let town = files.town_at(action.at);
            if matches!(action.hand.as_str(), "write" | "edit" | "apply_patch") {
                scan.events.push(SinceEvent {
                    hub: None,
                    id: identity("write", &[&address.key(), &path.to_string_lossy(), &number.to_string()]),
                    at: action.at,
                    machine: action.machine,
                    town,
                    sources: vec![scan.location(&path)],
                    what: SinceKind::FileAction {
                        resident: address.clone(),
                        hand: action.hand,
                        paths: action.what,
                        result: action.result,
                    },
                    source: None,
                });
                return;
            }
            if action.hand != "ask_colleague" {
                return;
            }
            match action.result.parse::<ResidentId>() {
                Ok(id) => scan.events.push(SinceEvent {
                    hub: None,
                    source: None,
                    id: identity("asked", &[&address.key(), &path.to_string_lossy(), &number.to_string()]),
                    at: action.at,
                    machine: scan.machine.clone(),
                    town,
                    sources: vec![scan.location(&path)],
                    what: SinceKind::ColleagueAsked {
                        asker: address.clone(),
                        colleague: Address { machine: action.machine, resident: id },
                        label: action.what,
                    },
                }),
                Err(error) => {
                    scan.issue(&path, format!("line {number}: cannot identify the invited colleague: {error}"))
                }
            }
        });
    }

    fn lines<T: serde::de::DeserializeOwned>(&mut self, path: &Path, mut take: impl FnMut(&mut Self, T, usize)) {
        let file = match std::fs::File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return,
            Err(error) => {
                self.issue(path, error);
                return;
            }
        };
        for (index, line) in io::BufReader::new(file).lines().enumerate() {
            match line.and_then(|line| serde_json::from_str(&line).map_err(io::Error::other)) {
                Ok(value) => take(self, value, index + 1),
                Err(error) => self.issue(path, format!("line {}: {error}", index + 1)),
            }
        }
    }

    fn turn(&mut self, files: &mut ResidentFiles, id: String, mut incoming: Turn) {
        let Some(prior) = files.turns.get_mut(&id) else {
            files.turns.insert(id, incoming);
            return;
        };
        // The records of one turn's end are written moments apart, so their
        // times differ by design; only a different outcome is a disagreement.
        if prior.end != incoming.end {
            self.issues.push(SinceIssue { source: incoming.sources[0].path.clone(), reason: format!("records of turn {id} disagree; the terminal record takes precedence over message rows and session events") });
        }
        if let Some(readers) = &mut prior.readers
            && let Some(more) = &incoming.readers
        {
            readers.extend_from_slice(more);
            readers.sort();
            readers.dedup();
        }
        if incoming.rank > prior.rank {
            if let (Some(readers), Some(more)) = (&mut incoming.readers, &prior.readers) {
                readers.extend_from_slice(more);
                readers.sort();
                readers.dedup();
            }
            std::mem::swap(prior, &mut incoming);
        }
        for source in incoming.sources {
            if !prior.sources.contains(&source) {
                prior.sources.push(source);
            }
        }
    }

    fn finish_resident(&mut self, id: &ResidentId, files: &ResidentFiles) {
        let about = files.about.as_ref().expect("a resident has an about record");
        let address = Address { machine: self.machine.clone(), resident: id.clone() };
        self.events.push(SinceEvent {
            hub: None,
            source: None,
            id: identity("born", &[&address.key()]),
            at: about.born_at,
            machine: self.machine.clone(),
            town: files.town_at(about.born_at),
            sources: files.birth_sources.clone(),
            what: SinceKind::ResidentBorn {
                resident: address.clone(),
                host: about.host.clone(),
                origin: about.origin.clone(),
            },
        });
        for (turn, finished) in &files.turns {
            self.events.push(SinceEvent {
                hub: None,
                source: None,
                id: identity("turn-ended", &[&address.key(), turn]),
                at: finished.at,
                machine: self.machine.clone(),
                town: files.town_at(finished.at),
                sources: finished.sources.clone(),
                what: SinceKind::TurnEnded {
                    resident: address.clone(),
                    turn: turn.clone(),
                    end: finished.end.clone(),
                    readers: finished.readers.clone(),
                },
            });
        }
        for (delivery, uncertain) in &files.uncertain {
            let place = uncertain.at.map_or_else(|| files.town_now(), |at| files.town_at(at));
            match uncertain.at {
                Some(at) => self.events.push(SinceEvent {
                    hub: None,
                    source: None,
                    id: identity("submission-uncertain", &[&address.key(), delivery.as_str()]),
                    at,
                    machine: self.machine.clone(),
                    town: place,
                    sources: uncertain.sources.clone(),
                    what: SinceKind::SubmissionUncertain {
                        resident: address.clone(),
                        delivery: delivery.clone(),
                        why: uncertain.why.clone(),
                        end: uncertain.end.clone(),
                        readers: uncertain.readers.clone(),
                    },
                }),
                None => self.undated.push(UndatedFact {
                    hub: None,
                    resident: address.clone(),
                    town: place,
                    source: uncertain.sources[0].clone(),
                    what: UndatedKind::SubmissionUncertain {
                        delivery: delivery.clone(),
                        why: uncertain.why.clone(),
                        end: uncertain.end.clone(),
                        readers: uncertain.readers.clone(),
                    },
                }),
            }
        }
    }

    fn claude(&mut self, world: &WorldDir, residents: &BTreeMap<ResidentId, ResidentFiles>) {
        for directory in self.dirs(&world.path().join("claude")) {
            let Some(resident) = self.optional::<ResidentId>(&directory.join("resident.json")) else { continue };
            let Some(files) = residents.get(&resident) else { continue };
            let address = Address { machine: self.machine.clone(), resident };
            let place = files.town_now();
            let path = directory.join("session.json");
            if let Some(registration) = self.optional::<fastctx_hosts::claude::exchange::Registration>(&path) {
                if let Some(at) = registration.at {
                    let place = files.town_at(at);
                    self.events.push(SinceEvent {
                        hub: None,
                        source: None,
                        id: identity(
                            "claude-registered",
                            &[
                                &address.key(),
                                &registration.session,
                                registration.instance.as_deref().unwrap_or(""),
                                &at.to_string(),
                            ],
                        ),
                        at,
                        machine: self.machine.clone(),
                        town: place.clone(),
                        sources: vec![self.location(&path)],
                        what: SinceKind::ClaudeRegistered {
                            resident: address.clone(),
                            session: registration.session,
                            instance: registration.instance,
                        },
                    });
                } else {
                    self.undated.push(UndatedFact {
                        hub: None,
                        resident: address.clone(),
                        town: place.clone(),
                        source: self.location(&path),
                        what: UndatedKind::ClaudeRegistered {
                            session: registration.session,
                            instance: registration.instance,
                        },
                    });
                }
            }
            for path in self.files(&directory.join("closed")) {
                self.undated.push(UndatedFact {
                    hub: None,
                    resident: address.clone(),
                    town: place.clone(),
                    source: self.location(&path),
                    what: UndatedKind::ClaudeClosed {
                        session: directory.file_name().unwrap().to_string_lossy().into_owned(),
                        instance: path.file_name().unwrap().to_string_lossy().into_owned(),
                    },
                });
            }
        }
    }

    fn ledger(&mut self, world: &WorldDir, town: &TownName) {
        let place = match crate::ledger::names::place(world, town) {
            Ok(Some(place)) => place,
            Ok(None) => return,
            Err(error) => {
                self.issue(&world.town(town).path().join("project.json"), error);
                return;
            }
        };
        for path in self.json_files(&world.town(town).signed()) {
            let Some(signed) = self.optional::<Signed>(&path) else { continue };
            match signed.claim::<Entry>() {
                Ok(entry) if entry.hub == place.hub && entry.town == place.town => {
                    self.ledger_entry(town, &path, entry)
                }
                Ok(_) => self.issue(&path, "entry belongs to another project"),
                Err(error) => self.issue(&path, error),
            }
        }
    }

    // These are the signed payloads checked on intake, as read by
    // ledger::entries_after. The editable projections are not evidence.
    fn ledger_entry(&mut self, town: &TownName, path: &Path, entry: Entry) {
        for file in entry.files {
            match ledger_fact(town, &file.path, &file.bytes.0) {
                Ok(Some((at, machine, what))) => self.events.push(SinceEvent {
                    hub: Some(entry.hub.clone()),
                    source: None,
                    id: identity("ledger-fact", &[town.as_str(), &file.path]),
                    at,
                    machine,
                    town: known(Some(town)),
                    sources: vec![self.location(path)],
                    what,
                }),
                Ok(None) => {}
                Err(error) => self.issue(path, format!("{}: {error}", file.path)),
            }
        }
    }
}

fn ledger_fact(_town: &TownName, path: &str, bytes: &[u8]) -> io::Result<Option<(Timestamp, MachineName, SinceKind)>> {
    let parts: Vec<_> = path.split('/').collect();
    let fact = match parts.as_slice() {
        ["project", version] if version_of(version).is_some() => {
            let record: Record = serde_json::from_slice(bytes).map_err(io::Error::other)?;
            (
                record.at,
                record.on,
                SinceKind::ProjectVersion { version: record.version, name: record.name, by: record.by },
            )
        }
        _ => return Ok(None),
    };
    Ok(Some(fact))
}

fn paths(directory: &Path) -> io::Result<Vec<PathBuf>> {
    match std::fs::read_dir(directory) {
        Ok(entries) => {
            let mut paths = entries.map(|entry| entry.map(|entry| entry.path())).collect::<io::Result<Vec<_>>>()?;
            paths.sort();
            Ok(paths)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error),
    }
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> io::Result<T> {
    serde_json::from_slice(&std::fs::read(path)?).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}
