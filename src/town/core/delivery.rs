//! Words on their way to residents: speaking to one, delivering by whatever route its host
//! offers, the durable queue for residents without a channel, and the notice that a command
//! ended.

use super::{Delivery, MessageLine, Route, Spoken, Town, delivery_id, format_duration, mod_delivery, set_state};
use crate::town::codex::host::{Input, Landed};
use crate::town::talk::TalkEvent;
use std::path::{Path, PathBuf};

impl Town {
    /// Speaks to a resident on this machine on behalf of `from`. For a Codex resident, `from`
    /// then receives the last message of the turn its words went into.
    pub(crate) async fn speak(&self, from: &str, to: &str, message: &str) -> Result<Spoken, String> {
        let route = {
            let state = self.state.lock();
            let live = state.residents.get(to).ok_or_else(|| format!("No resident named {to} lives on {}.", self.config.machine))?;
            let _ = live.records.append_message(&MessageLine {
                at: crate::world::now_rfc3339(),
                kind: "said".into(),
                peer: from.to_string(),
                turn: None,
                text: Some(message.to_string()),
            });
            live.route.clone()
        };
        let text = format!("=== from {from} ===\n{message}");
        match route {
            Route::CodexThread(thread) => {
                // A Codex thread echoes its input as an item, which watchers see.
                let _turns = self.turns.lock().await;
                let (landed, turn) = self.codex.send(&thread, Input::Text(text)).await?;
                if let Some(live) = self.state.lock().residents.get_mut(to) {
                    if landed == Landed::NewTurn {
                        set_state(&live.records, "busy");
                    }
                    live.readers.entry(turn).or_default().insert(from.to_string());
                }
                Ok(match landed {
                    Landed::NewTurn => Spoken::NewTurn,
                    Landed::Steered => Spoken::JoinedTurn,
                })
            }
            Route::ClaudeMod(_) | Route::Piggyback => {
                self.publish(to, TalkEvent::Words { from: from.to_string(), text: message.to_string() });
                let delivery = Delivery { id: delivery_id()?, from: from.to_string(), kind: "message".into(), text, as_tool_output: false };
                self.deliver(to, delivery).await?;
                Ok(Spoken::Queued)
            }
        }
    }

    /// Puts the user's own words into a resident's session, as the user. Only words the user
    /// typed on one of the user's devices come here (`specs/world.md`, the first rule).
    /// Returns how they landed and, for a Codex resident, the turn they went into.
    pub(crate) async fn say_as_user(&self, id: &str, text: &str) -> Result<(Spoken, Option<String>), String> {
        let route = self.state.lock().residents.get(id).map(|live| live.route.clone());
        let mut turn = None;
        let spoken = match route {
            Some(Route::CodexThread(thread)) => {
                let (landed, into) = self.codex.send(&thread, Input::Text(text.to_string())).await?;
                turn = Some(into);
                match landed {
                    Landed::NewTurn => {
                        if let Some(live) = self.state.lock().residents.get(id) {
                            set_state(&live.records, "busy");
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
            self.publish(id, TalkEvent::Words { from: "user".into(), text: text.to_string() });
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
        std::fs::create_dir_all(&dir).map_err(|error| format!("Cannot create {}: {error}", crate::paths::display_path(&dir)))?;
        let bytes = serde_json::to_vec(&delivery).map_err(|error| format!("Cannot encode a delivery: {error}"))?;
        crate::world::write_atomic(&dir.join(format!("{}.json", delivery.id)), &bytes)
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

    /// A command a resident started on this machine has ended: the resident that started it is
    /// told, and only it (`specs/world-run.md`, "怎样得知它跑完了"). The notice reaches a Codex
    /// resident as a tool output, a fact rather than words to answer.
    pub(crate) async fn job_ended(&self, record: &Path) -> Result<(), String> {
        let jobs = record.parent().ok_or("A job record sits in a jobs directory.")?;
        let resident_dir = jobs.parent().ok_or("A jobs directory sits in a resident's directory.")?;
        let resident = resident_dir.file_name().and_then(|name| name.to_str()).ok_or("A resident directory is named by its id.")?;
        let job = crate::town::who::read_job(record, resident, None)
            .ok_or_else(|| format!("{} is not a job record.", crate::paths::display_path(record)))?;
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
        if !self.state.lock().residents.contains_key(resident) {
            // Its starter lives on another machine, or has gone; the record says what happened.
            return Ok(());
        }
        let delivery = Delivery { id: delivery_id()?, from: "world".into(), kind: "notice".into(), text, as_tool_output: true };
        self.deliver(resident, delivery).await
    }
}

/// Moves a resident's waiting words to its new records directory when it changes towns.
pub(super) fn move_waiting(from: &Path, to: &Path) -> Result<(), String> {
    let source = from.join("inbox");
    let entries = match std::fs::read_dir(&source) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("Cannot list {}: {error}", crate::paths::display_path(&source))),
    };
    let target = to.join("inbox");
    std::fs::create_dir_all(&target).map_err(|error| format!("Cannot create {}: {error}", crate::paths::display_path(&target)))?;
    for entry in entries.flatten() {
        let destination = target.join(entry.file_name());
        std::fs::rename(entry.path(), &destination)
            .map_err(|error| format!("Cannot move {}: {error}", crate::paths::display_path(&entry.path())))?;
    }
    Ok(())
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
