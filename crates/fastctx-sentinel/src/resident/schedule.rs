//! Timer reconciliation and offline reflex suspension.
use super::*;
use storage::Offline;

impl Host {
    fn timer_owner(&self, registration: &Registration) -> String {
        format!(
            "sentinel:{}:{}",
            self.assignment(&registration.scope, &registration.definition.id)
                .parent()
                .expect("assignment directory")
                .to_string_lossy(),
            registration.definition.generation
        )
    }
    fn desired(&self, registration: &Registration, state: &State, ledger: &Ledger) -> Vec<Timer> {
        let mut result = Vec::new();
        if state.finished || !registration.active || state.definition.kind == Kind::Goal {
            return result;
        }
        let d = &registration.definition;
        let owner = self.timer_owner(registration);
        for (index, at) in d.at.iter().enumerate().filter(|(index, _)| !ledger.alarms.contains(index)) {
            result.push(Timer {
                id: format!("{owner}:alarm:{index}"),
                owner: owner.clone(),
                at: *at,
                action: TimerAction::WakeSentinel {
                    scope: registration.scope.clone(),
                    id: d.id.clone(),
                    generation: d.generation,
                    wake: ScheduledWake::OwnerAlarm { index },
                },
            });
        }
        for r in state.reflexes.iter().filter(|r| r.at < i64::MAX) {
            let at = ledger.offline.get(&r.id).map_or(r.at, |o| o.check_at);
            let action = if r.probe.is_none() {
                TimerAction::WakeSentinel {
                    scope: registration.scope.clone(),
                    id: d.id.clone(),
                    generation: d.generation,
                    wake: ScheduledWake::SentinelTime { reflex: r.id.clone() },
                }
            } else {
                TimerAction::RunReflex {
                    scope: registration.scope.clone(),
                    id: d.id.clone(),
                    generation: d.generation,
                    reflex: r.id.clone(),
                }
            };
            // A later schedule uses a distinct ID: an old firing's acknowledgement
            // cannot remove a replacement installed while the callback was running.
            result.push(Timer { id: format!("{owner}:reflex:{}:{at}", r.id), owner: owner.clone(), at, action });
        }
        result
    }
    pub(super) fn accepts(&self, registration: &Registration, state: &State, ledger: &Ledger, fired: &Fired) -> bool {
        self.desired(registration, state, ledger)
            .iter()
            .any(|t| t == &fired.timer && t.at == fired.scheduled_at && t.at <= self.runtime.now())
    }
    pub(super) async fn synchronize(
        &self,
        registration: &Registration,
        state: &State,
        ledger: &Ledger,
    ) -> Result<(), String> {
        tokio::time::timeout(std::time::Duration::from_secs(60), async {
            let desired = self.desired(registration, state, ledger);
            let current = self.timers.list(&self.timer_owner(registration)).await?;
            for timer in &desired {
                if !current.contains(timer) {
                    self.timers.set(timer).await?;
                }
            }
            for timer in current {
                if !desired.contains(&timer) {
                    self.timers.cancel(&timer.id).await?;
                }
            }
            Ok(())
        })
        .await
        .map_err(|_| "reconciling sentinel timers timed out")?
    }
    pub(super) async fn check_reflex(
        &self,
        store: &Store,
        state: &mut State,
        ledger: &mut Ledger,
        fired: Option<&Fired>,
    ) -> Result<Option<(String, WakeReason)>, String> {
        let now = self.runtime.now();
        let registration: Registration = fastctx_agent::state::load(
            &store.directory().parent().expect("assignment directory").join("assignment.json"),
        )?
        .ok_or("sentinel assignment missing")?;
        let wanted = match fired.map(|f| &f.timer.action) {
            Some(TimerAction::RunReflex { reflex, .. })
            | Some(TimerAction::WakeSentinel { wake: ScheduledWake::SentinelTime { reflex }, .. }) => {
                Some(reflex.as_str())
            }
            Some(_) => return Ok(None),
            None => None,
        };
        for index in 0..state.reflexes.len() {
            let r = &state.reflexes[index];
            if r.at > now || wanted.is_some_and(|wanted| wanted != r.id) {
                continue;
            }
            let identity = format!("reflex-{}-{}", r.id, r.at);
            let status = match &r.probe {
                Some(Probe::Look { machine, .. }) => Some(self.runtime.online(&registration, machine).await),
                _ => None,
            };
            let reason = match status {
                Some(Ok(false)) => {
                    let offline = ledger
                        .offline
                        .entry(r.id.clone())
                        .or_insert(Offline { since: now, check_at: now.saturating_add(86_400_000) });
                    if now.saturating_sub(offline.since) < 86_400_000 {
                        ledger.save(store)?;
                        continue;
                    }
                    Some(WakeReason::Broken {
                        id: r.id.clone(),
                        reason: "the observed machine has been offline for more than a day".into(),
                    })
                }
                Some(Err(reason)) => Some(WakeReason::Broken { id: r.id.clone(), reason }),
                _ => {
                    ledger.offline.remove(&r.id);
                    let _permit = tokio::time::timeout(std::time::Duration::from_secs(30), self.running.acquire())
                        .await
                        .map_err(|_| "the sentinel home is busy")?
                        .map_err(|e| e.to_string())?;
                    let capabilities = self.runtime.capabilities(&registration, store.directory()).await?;
                    match crate::reflex::evaluate(
                        &state.definition,
                        &mut state.reflexes[index],
                        now,
                        capabilities.look.as_ref(),
                        capabilities.web.as_ref(),
                    )
                    .await
                    {
                        crate::reflex::Evaluation::Quiet => None,
                        crate::reflex::Evaluation::Wake(reason) => Some(reason),
                    }
                }
            };
            ledger.save(store)?;
            if let Some(reason) = reason {
                let reflex = &mut state.reflexes[index];
                if reflex.at <= now {
                    reflex.at = reflex.every_ms.map_or(i64::MAX, |period| now.saturating_add(period as i64));
                }
                state.pending =
                    Some(crate::state::WakeIntent { id: identity.clone(), reason: reason.clone(), at: now });
                store.checkpoint(state)?;
                return Ok(Some((identity, reason)));
            }
            store.checkpoint(state)?;
            return Ok(None);
        }
        Ok(None)
    }
}
