//! Words on their way to residents: speaking to one, delivering by whatever route its host
//! offers, the durable queue for residents without a channel, and the notice that a command
//! ended.

use super::{Delivery, MessageLine, Route, Spoken, Town, delivery_id, format_duration, mod_delivery};
use crate::town::codex::host::{Input, Landed};
use crate::town::remote::{Letter, Peer};
use crate::town::talk::TalkEvent;
use std::path::{Path, PathBuf};

impl Town {
    /// How a resident is named to residents of this machine: its id, and its machine when that
    /// is another one.
    pub(crate) fn peer_name(&self, peer: &Peer) -> String {
        if peer.machine == self.config.machine { peer.id.clone() } else { format!("{} on {}", peer.id, peer.machine) }
    }

    /// Speaks to a resident on this machine on behalf of `from`, who lives here or elsewhere.
    /// For a Codex resident, `from` then receives the last message of the turn its words went
    /// into.
    pub(crate) async fn speak(&self, from: &Peer, to: &str, message: &str) -> Result<Spoken, String> {
        let name = self.peer_name(from);
        let route = {
            let mut state = self.state.lock();
            // A requester that speaks again is back.
            state.departed.remove(from);
            let live = state.residents.get(to).ok_or_else(|| format!("No resident named {to} lives on {}.", self.config.machine))?;
            let _ = live.records.append_message(&MessageLine {
                at: crate::world::now_rfc3339(),
                kind: "said".into(),
                peer: name.clone(),
                turn: None,
                text: Some(message.to_string()),
            });
            live.route.clone()
        };
        let text = format!("=== from {name} ===\n{message}");
        match route {
            Route::CodexThread(thread) => {
                // A Codex thread echoes its input as an item, which watchers see.
                let _turns = self.turns.lock().await;
                let (landed, turn) = self.codex.send(&thread, Input::Text(text)).await?;
                if let Some(live) = self.state.lock().residents.get_mut(to) {
                    if landed == Landed::NewTurn {
                        self.set_state(&live.records, "busy");
                    }
                    live.turn = Some(turn.clone());
                    live.readers.entry(turn).or_default().insert(from.clone());
                }
                Ok(match landed {
                    Landed::NewTurn => Spoken::NewTurn,
                    Landed::Steered => Spoken::JoinedTurn,
                })
            }
            Route::ClaudeMod(_) | Route::Piggyback => {
                self.publish(to, TalkEvent::Words { from: name.clone(), text: message.to_string(), item: None });
                let delivery = Delivery { id: delivery_id()?, from: name, kind: "message".into(), text, as_tool_output: false };
                self.deliver(to, delivery).await?;
                Ok(Spoken::Queued)
            }
        }
    }

    /// Mails a letter to another machine's town. It is committed here before this returns, and
    /// reaches that machine when it is online, in the order letters were sent.
    pub(crate) fn mail(&self, machine: &str, letter: &Letter) -> Result<(), String> {
        let body = serde_json::to_value(letter).map_err(|error| format!("Cannot encode a letter: {error}"))?;
        self.world.send_mail(machine, body).map(|_| ())
    }

    /// Delivers words that came for a resident of this machine from elsewhere. A resident that
    /// is not here, or whose host cannot take them now, keeps them waiting in its records until
    /// its next World tool response or its return.
    pub(crate) async fn deliver_or_hold(&self, to: &str, delivery: Delivery) -> Result<(), String> {
        if self.state.lock().residents.contains_key(to) {
            match self.deliver(to, delivery.clone()).await {
                Ok(()) => return Ok(()),
                Err(error) => (self.log)(format!("cannot deliver to {to} now, so it waits in its records: {error}")),
            }
        }
        let _inbox = self.inbox.lock();
        let dir = match self.inbox_dir(to) {
            Some(dir) => dir,
            None => self
                .current_records(to)
                .map(|(records, _)| records.dir.join("inbox"))
                .ok_or_else(|| format!("No resident named {to} has records on {}.", self.config.machine))?,
        };
        write_waiting(&dir, &delivery)
    }

    /// Puts the user's own words into a resident's session, as the user. Only words the user
    /// typed on one of the user's devices come here (`specs/world.md`, the first rule).
    /// Returns how they landed and, for a Codex resident, the turn they went into.
    pub(crate) async fn say_as_user(&self, id: &str, text: &str) -> Result<(Spoken, Option<String>), String> {
        let route = self.state.lock().residents.get(id).map(|live| live.route.clone());
        let mut turn = None;
        let spoken = match route {
            Some(Route::CodexThread(thread)) => {
                let _turns = self.turns.lock().await;
                let (landed, into) = self.codex.send(&thread, Input::Text(text.to_string())).await?;
                turn = Some(into.clone());
                match landed {
                    Landed::NewTurn => {
                        if let Some(live) = self.state.lock().residents.get_mut(id) {
                            self.set_state(&live.records, "busy");
                            live.turn = Some(into.clone());
                        }
                        Spoken::NewTurn
                    }
                    Landed::Steered => Spoken::JoinedTurn,
                }
            }
            Some(Route::ClaudeMod(Some(session))) => {
                let delivery = crate::town::claude::ModDelivery {
                    id: delivery_id()?,
                    from: "user".into(),
                    kind: "typed".into(),
                    text: text.to_string(),
                    as_user: true,
                };
                crate::town::claude::deliver(&self.config.world_dir, &session, &delivery)?;
                Spoken::Queued
            }
            Some(_) => return Err(format!("{id}'s host offers the World no way to put words into its session.")),
            None => return Err(format!("No resident named {id} lives on {}.", self.config.machine)),
        };
        if let Some(live) = self.state.lock().residents.get(id) {
            let _ = live.records.append_message(&MessageLine {
                at: crate::world::now_rfc3339(),
                kind: "said".into(),
                peer: "user".into(),
                turn: None,
                text: Some(text.to_string()),
            });
        }
        if spoken == Spoken::Queued {
            self.publish(id, TalkEvent::Words { from: "user".into(), text: text.to_string(), item: None });
        }
        Ok((spoken, turn))
    }

    /// Delivers words to a resident on this machine by whatever route its host offers.
    pub(crate) async fn deliver(&self, to: &str, delivery: Delivery) -> Result<(), String> {
        let (route, about) = {
            let state = self.state.lock();
            match state.residents.get(to) {
                Some(live) => (live.route.clone(), live.about.clone()),
                None => return Err(format!("No resident named {to} lives on {}.", self.config.machine)),
            }
        };
        match route {
            Route::CodexThread(thread) => {
                let input = if delivery.as_tool_output {
                    Input::ToolOutput { name: "world".into(), namespace: Some("fastctx".into()), output: delivery.text }
                } else {
                    Input::Text(delivery.text)
                };
                self.codex.send(&thread, input).await.map(|_| ())
            }
            Route::ClaudeMod(Some(session)) => {
                // A background resident whose process the supervisor stopped is woken with the
                // words; any other Claude session reads them from its mod's inbox.
                if about.surface == "bg" && !crate::town::claude::background_running(&session).await? {
                    let cwd = self.standing_dir(about.town.as_deref());
                    return crate::town::claude::wake(&session, &delivery.text, &cwd).await;
                }
                crate::town::claude::deliver(&self.config.world_dir, &session, &mod_delivery(delivery))
            }
            Route::ClaudeMod(None) | Route::Piggyback => self.enqueue(to, delivery),
        }
    }

    /// Takes everything waiting for a resident, oldest first; the caller delivers it.
    pub(crate) fn take_waiting(&self, id: &str) -> Vec<Delivery> {
        let _inbox = self.inbox.lock();
        let Some(dir) = self.inbox_dir(id) else { return Vec::new() };
        let mut taken = Vec::new();
        for (path, delivery) in waiting(&dir) {
            match std::fs::remove_file(&path) {
                Ok(()) => taken.push(delivery),
                Err(error) => (self.log)(format!("cannot take the waiting {}: {error}", crate::paths::display_path(&path))),
            }
        }
        taken
    }

    fn enqueue(&self, to: &str, delivery: Delivery) -> Result<(), String> {
        let _inbox = self.inbox.lock();
        let dir = self.inbox_dir(to).ok_or_else(|| format!("No resident named {to} lives on {}.", self.config.machine))?;
        write_waiting(&dir, &delivery)
    }

    /// Moves what waits for a resident into its Claude session's inbox, once its mod has
    /// registered. Each delivery leaves the queue only after it is in the session's inbox.
    pub(super) fn migrate_waiting(&self, id: &str, session: &str) {
        let _inbox = self.inbox.lock();
        let Some(dir) = self.inbox_dir(id) else { return };
        for (path, delivery) in waiting(&dir) {
            match crate::town::claude::deliver(&self.config.world_dir, session, &mod_delivery(delivery)) {
                Ok(()) => {
                    if let Err(error) = std::fs::remove_file(&path) {
                        (self.log)(format!("cannot remove the moved {}: {error}", crate::paths::display_path(&path)));
                    }
                }
                Err(error) => (self.log)(format!("cannot move a waiting delivery into {id}'s session: {error}")),
            }
        }
    }

    fn inbox_dir(&self, id: &str) -> Option<PathBuf> {
        self.state.lock().residents.get(id).map(|live| live.records.dir.join("inbox"))
    }

    /// `run` gave a command back while it still ran, so its end is owed to the resident that
    /// started it. A marker beside the record says so, and names the machine that resident
    /// lives on when it is not this one. Whoever claims the marker first delivers the notice:
    /// the command's end hook, this look if the command ended before the marker was written,
    /// or the node's next start if the node was down when it ended. So a notice is given once,
    /// never lost, and never given for a command whose end its caller already saw.
    pub(crate) async fn owe_end(&self, record: &Path, home: Option<&str>) -> Result<(), String> {
        let marker = record.with_extension(OWED);
        std::fs::write(&marker, home.unwrap_or_default())
            .map_err(|error| format!("Cannot write {}: {error}", crate::paths::display_path(&marker)))?;
        self.job_ended(record).await
    }

    /// Delivers every owed end recorded in one resident's directory whose command has ended.
    /// At the node's start a claim left by a delivery that never finished is owed again; no
    /// delivery is running then.
    pub(crate) async fn owed_ends(&self, resident_dir: &Path, starting: bool) {
        let jobs = resident_dir.join("jobs");
        let markers: Vec<PathBuf> = std::fs::read_dir(&jobs).into_iter().flatten().flatten().map(|entry| entry.path()).collect();
        for claim in markers.iter().filter(|path| starting && path.extension().is_some_and(|extension| extension == CLAIMED)) {
            if let Err(error) = std::fs::rename(claim, claim.with_extension(OWED)) {
                (self.log)(format!("cannot owe {} again: {error}", crate::paths::display_path(claim)));
            }
        }
        let owed: Vec<PathBuf> = std::fs::read_dir(&jobs)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|extension| extension == OWED))
            .collect();
        for marker in owed {
            if let Err(error) = self.job_ended(&marker.with_extension("json")).await {
                (self.log)(format!("cannot deliver the end of {}: {error}", crate::paths::display_path(&marker)));
            }
        }
    }

    /// A command a resident started on this machine has ended: if its end is owed, the resident
    /// that started it is told, and only it (`specs/world-run.md`, "怎样得知它跑完了"). The notice
    /// reaches a Codex resident as a tool output, a fact rather than words to answer. A
    /// resident of this machine that is not here keeps its notice owed until it comes back; one
    /// living elsewhere is mailed, and its machine holds the notice for it.
    pub(crate) async fn job_ended(&self, record: &Path) -> Result<(), String> {
        let jobs = record.parent().ok_or("A job record sits in a jobs directory.")?;
        let resident_dir = jobs.parent().ok_or("A jobs directory sits in a resident's directory.")?;
        let resident = resident_dir.file_name().and_then(|name| name.to_str()).ok_or("A resident directory is named by its id.")?;
        let job = crate::town::who::read_job(record, resident, None)
            .ok_or_else(|| format!("{} is not a job record.", crate::paths::display_path(record)))?;
        let marker = record.with_extension(OWED);
        let home = match std::fs::read_to_string(&marker) {
            Ok(home) => Some(home).filter(|home| !home.is_empty() && *home != self.config.machine),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(format!("Cannot read {}: {error}", crate::paths::display_path(&marker))),
        };
        if matches!(job.state.as_str(), "starting" | "running") || (home.is_none() && !self.state.lock().residents.contains_key(resident)) {
            return Ok(());
        }
        let claimed = record.with_extension(CLAIMED);
        match std::fs::rename(&marker, &claimed) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(format!("Cannot claim {}: {error}", crate::paths::display_path(&marker))),
        }
        let ended = match (job.state.as_str(), job.exit_code) {
            ("exited", Some(code)) => format!("exited {code}"),
            ("interrupted", _) => "was interrupted".to_string(),
            (state, _) => state.to_string(),
        };
        let rfc3339 = &time::format_description::well_known::Rfc3339;
        let elapsed = match (time::OffsetDateTime::parse(&job.started_at, rfc3339), job.ended_at.as_deref().map(|at| time::OffsetDateTime::parse(at, rfc3339))) {
            (Ok(start), Some(Ok(end))) => format_duration(u64::try_from((end - start).whole_milliseconds()).unwrap_or_default()),
            _ => "?".to_string(),
        };
        let command = job.command.lines().find(|line| !line.trim().is_empty()).unwrap_or_default().trim().to_string();
        let log = crate::paths::display_path(&job.output_path);
        let text = crate::hands::text::job_ended_notice(&job.id, &job.machine, &ended, &elapsed, &command, &log);
        let delivered = match delivery_id() {
            Ok(id) => {
                let delivery = Delivery { id, from: "world".into(), kind: "notice".into(), text, as_tool_output: true };
                match &home {
                    None => self.deliver(resident, delivery).await,
                    Some(home) => self.mail(home, &Letter::Deliver { to: resident.to_string(), delivery }),
                }
            }
            Err(error) => Err(error),
        };
        // Delivered, the claim becomes the record that it was; failed, it is owed again. A
        // delivery cut off between the two leaves the claim, owed again at the node's start.
        let settled = if delivered.is_ok() { record.with_extension(DELIVERED) } else { marker };
        if let Err(error) = std::fs::rename(&claimed, &settled) {
            (self.log)(format!("cannot settle {}: {error}", crate::paths::display_path(&claimed)));
        }
        delivered
    }
}

/// The marker of an end owed to the resident that started a command, the name it takes while
/// the notice is on its way, and the name it keeps once the notice went out.
const OWED: &str = "notify";
const CLAIMED: &str = "notifying";
const DELIVERED: &str = "notified";

impl Town {
    /// At the node's start: commands whose supervisor did not survive, because the machine
    /// restarted, are recorded as interrupted (`specs/world-run.md`, "跑起来的东西留在世界上"),
    /// and every end still owed, including those that came while the node was down, is told.
    pub(super) async fn reconcile_jobs(self: std::sync::Arc<Self>) {
        for dir in self.record_dirs() {
            let scanned = dir.clone();
            match tokio::task::spawn_blocking(move || crate::run::reconcile(&scanned)).await {
                Ok(Ok(_)) => {}
                Ok(Err(error)) => (self.log)(format!("cannot reconcile the commands in {}: {error}", crate::paths::display_path(&dir))),
                Err(error) => (self.log)(format!("reconciling {} stopped: {error}", crate::paths::display_path(&dir))),
            }
            self.owed_ends(&dir, true).await;
        }
    }
}

/// Moves a resident's waiting words to its new records directory when it changes towns: all
/// of them or none, so every word waits in one place.
pub(super) fn move_waiting(from: &Path, to: &Path) -> Result<(), String> {
    let source = from.join("inbox");
    let names: Vec<std::ffi::OsString> = match std::fs::read_dir(&source) {
        Ok(entries) => entries.flatten().map(|entry| entry.file_name()).collect(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("Cannot list {}: {error}", crate::paths::display_path(&source))),
    };
    if names.is_empty() {
        return Ok(());
    }
    let target = to.join("inbox");
    std::fs::create_dir_all(&target).map_err(|error| format!("Cannot create {}: {error}", crate::paths::display_path(&target)))?;
    let mut moved = Vec::new();
    for name in names {
        if let Err(error) = std::fs::rename(source.join(&name), target.join(&name)) {
            let failure = format!("Cannot move {}: {error}", crate::paths::display_path(&source.join(&name)));
            for name in moved.iter().rev() {
                if let Err(back) = std::fs::rename(target.join(name), source.join(name)) {
                    return Err(format!("{failure}; and cannot move {} back: {back}", crate::paths::display_path(&target.join(name))));
                }
            }
            return Err(failure);
        }
        moved.push(name);
    }
    Ok(())
}

/// Leaves a delivery waiting in a resident's queue directory.
fn write_waiting(dir: &Path, delivery: &Delivery) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|error| format!("Cannot create {}: {error}", crate::paths::display_path(dir)))?;
    let bytes = serde_json::to_vec(delivery).map_err(|error| format!("Cannot encode a delivery: {error}"))?;
    crate::world::write_atomic(&dir.join(format!("{}.json", delivery.id)), &bytes)
}

/// The deliveries waiting in a queue directory, oldest first. A file that is not a delivery
/// stays where it is and is not taken.
fn waiting(dir: &Path) -> Vec<(PathBuf, Delivery)> {
    let mut waiting: Vec<(PathBuf, Delivery)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let delivery = serde_json::from_slice::<Delivery>(&std::fs::read(&path).ok()?).ok()?;
            Some((path, delivery))
        })
        .collect();
    waiting.sort_by(|left, right| left.1.id.cmp(&right.1.id));
    waiting
}
