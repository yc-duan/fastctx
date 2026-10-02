//! Claude residents: sessions that carry the World's mod. Starting background ones, pairing a
//! session's MCP connection with its mod's registration, and following what the mods report.

use super::{About, Birth, HashMap, Live, MessageLine, Now, Records, Route, Town, new_resident_id, resident_instructions};
use crate::town::claude::ModEvent;
use crate::town::talk::TalkEvent;
use serde_json::Value;
use std::sync::Arc;

impl Town {
    /// Starts a background Claude resident. Its mod registers with the resident id the World
    /// gave it, which pairs the session with these records.
    pub(super) async fn start_claude(&self, town: Option<String>, words: &str) -> Result<String, String> {
        if words.trim().is_empty() {
            return Err("A Claude resident starts with the user's first words; give them.".to_string());
        }
        let id = {
            let state = self.state.lock();
            new_resident_id("claude", |candidate| state.residents.contains_key(candidate) || self.recorded(candidate))?
        };
        let records = Records::new(self.resident_dir(town.as_deref(), &id));
        records.create()?;
        let session = crate::town::claude::new_session_id()?;
        let about = About {
            id: id.clone(),
            agent: "claude-code".to_string(),
            agent_version: None,
            model: None,
            model_source: None,
            machine: self.config.machine.clone(),
            surface: "bg".to_string(),
            birth: Birth::User,
            since: crate::world::now_rfc3339(),
            session: Some(session.clone()),
            town: town.clone(),
        };
        let cwd = self.standing_dir(town.as_deref());
        let instructions = resident_instructions(&about, &self.config.machine, self.map(town.as_deref()));
        crate::town::claude::start_background(&crate::town::claude::Background {
            resident: &id,
            session: &session,
            cwd: &cwd,
            records: &records.dir,
            world_dir: &self.config.world_dir,
            fastctx: &self.config.fastctx,
            home: &self.config.home,
            instructions: &instructions,
            words,
        })
        .await?;
        records.write_about(&about)?;
        records.write_now(&Now { state: "busy".into(), since: about.since.clone(), ..Now::default() })?;
        self.state.lock().residents.insert(
            id.clone(),
            Live { records, about, route: Route::ClaudeMod(None), inbox_address: None, sessions: 0, readers: HashMap::new() },
        );
        Ok(id)
    }

    /// Links Claude residents to the sessions their mods registered: by the resident id the
    /// World gave a session it launched, or else by the session's inbox address. A session
    /// already linked to a resident is not linked to another. Whatever waited for a resident
    /// before its pairing moves to its session's inbox.
    pub(super) fn pair_claude_sessions(&self) {
        let mut paired = Vec::new();
        {
            let mut state = self.state.lock();
            if !state.residents.values().any(|live| live.route == Route::ClaudeMod(None)) {
                return;
            }
            for session in crate::town::claude::sessions(&self.config.world_dir) {
                let taken = state.residents.values().any(|live| live.route == Route::ClaudeMod(Some(session.clone())));
                if taken {
                    continue;
                }
                let registration = match crate::town::claude::registration(&self.config.world_dir, &session) {
                    Ok(Some(registration)) => registration,
                    Ok(None) => continue,
                    Err(error) => {
                        (self.log)(error);
                        continue;
                    }
                };
                let found = state.residents.iter_mut().find(|(id, live)| {
                    live.route == Route::ClaudeMod(None)
                        && (registration.resident.as_deref() == Some(id.as_str())
                            || (registration.inbox_address.is_some() && registration.inbox_address == live.inbox_address))
                });
                if let Some((id, live)) = found {
                    live.route = Route::ClaudeMod(Some(session.clone()));
                    live.about.session = Some(session.clone());
                    let model = match &registration.model {
                        Some(Value::String(model)) => Some(model.clone()),
                        Some(Value::Object(model)) => model.get("id").and_then(Value::as_str).map(str::to_string),
                        _ => None,
                    };
                    if model.is_some() {
                        live.about.model = model;
                        live.about.model_source = Some("host".into());
                    }
                    if let Err(error) = live.records.write_about(&live.about) {
                        (self.log)(error);
                    }
                    paired.push((id.clone(), session));
                }
            }
        }
        for (id, session) in paired {
            self.migrate_waiting(&id, &session);
        }
    }

    /// Reads what the Claude mods wrote: pairs new sessions, turns their events into the
    /// residents' `now`, and records which deliveries reached the model.
    pub(super) async fn follow_claude(self: Arc<Self>) {
        // The mod looks at its inbox every 2 s; looking back at the same pace keeps `now` at
        // most one look behind what the mod reported.
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(2));
        loop {
            ticker.tick().await;
            self.pair_claude_sessions();
            let sessions: Vec<(String, Records)> = {
                let state = self.state.lock();
                state
                    .residents
                    .values()
                    .filter_map(|live| match &live.route {
                        Route::ClaudeMod(Some(session)) => Some((session.clone(), live.records.clone())),
                        _ => None,
                    })
                    .collect()
            };
            for (session, records) in sessions {
                let harvest = crate::town::claude::harvest(&self.config.world_dir, &session);
                if let Some(problem) = &harvest.problem {
                    (self.log)(problem.clone());
                }
                for event in &harvest.events {
                    if let Some(talk) = claude_event(event) {
                        self.publish_session(&session, talk);
                    }
                }
                for event in harvest.events {
                    let state = match event.kind.as_str() {
                        "turn_start" => "busy",
                        "turn_complete" => "idle",
                        "session_end" => "left",
                        _ => continue,
                    };
                    let mut now = records.now().ok().flatten().unwrap_or_default();
                    now.state = state.to_string();
                    now.since = event.at;
                    let _ = records.write_now(&now);
                }
                for delivered in harvest.delivered {
                    let _ = records.append_message(&MessageLine {
                        at: crate::world::now_rfc3339(),
                        kind: "read".into(),
                        peer: delivered,
                        turn: None,
                        text: None,
                    });
                }
            }
        }
    }
}

/// What a watcher sees of a Claude mod's event; the mod reports turns and tools, not text.
fn claude_event(event: &ModEvent) -> Option<TalkEvent> {
    match event.kind.as_str() {
        "turn_start" => Some(TalkEvent::TurnStarted { turn: event.at.clone() }),
        "turn_complete" => match &event.answer {
            Some(answer) => Some(TalkEvent::Message { item: event.at.clone(), text: answer.clone() }),
            None => Some(TalkEvent::TurnEnded {
                turn: event.at.clone(),
                status: if event.aborted == Some(true) { "interrupted".into() } else { "completed".into() },
            }),
        },
        "tool" => Some(TalkEvent::Tool {
            item: event.at.clone(),
            name: event.tool.clone().unwrap_or_default(),
            summary: String::new(),
            done: false,
            failed: false,
        }),
        "session_end" => Some(TalkEvent::State { state: "left".into() }),
        _ => None,
    }
}
