//! Serialized, replayable goal judgments on the assigned home.
use super::*;
use fastctx_agent::state::load;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Serialize, Deserialize)]
struct Receipt {
    input: String,
    judgment: Option<Judgment>,
}
impl Host {
    /// The engine owns goal state and notification. This only returns a judgment.
    pub async fn judge(&self, scope: &HubAccount, request: &GoalRequest) -> Result<Judgment, String> {
        if request.request_id.trim().is_empty() || request.request_id.len() > 256 {
            return Err("invalid goal review ID".into());
        }
        let input = crate::goal::input(&request.review)?.to_string();
        let gate = self.gate(scope, &request.id);
        let _guard = gate.lock().await;
        let (_, registration) = lifecycle::get(self.records.as_ref(), scope, &request.id)
            .await
            .map_err(|e| e.to_string())?
            .ok_or("goal sentinel is no longer registered")?;
        if !registration.active
            || registration.definition.kind != Kind::Goal
            || registration.definition.generation != request.generation
            || registration.definition.owner != request.owner
            || registration.definition.condition != request.review.goal
        {
            return Err("goal review does not match its active sentinel assignment".into());
        }
        if !self.runtime.assigned(&registration).await? {
            return Err("goal sentinel is assigned to another home".into());
        }
        self.reconcile_locked(scope, &request.id, None).await?;
        let store = self.store(&registration);
        let key = format!("{:x}", Sha256::digest(request.request_id.as_bytes()));
        let receipt_path = store.directory().join("judgments").join(format!("{key}.json"));
        if let Some(receipt) = load::<Receipt>(&receipt_path)? {
            if receipt.input != input {
                return Err("goal review ID was reused with different material".into());
            }
            if let Some(judgment) = receipt.judgment {
                return Ok(judgment);
            }
        } else {
            save(&receipt_path, &Receipt { input: input.clone(), judgment: None })?;
        }
        let wake_id = format!("goal-{key}");
        let mut state = store.load()?.ok_or("goal sentinel state is missing")?;
        if state.wake_id != wake_id {
            let entries = match std::fs::read_dir(store.directory().join("history")) {
                Ok(entries) => Some(entries),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => return Err(error.to_string()),
            };
            if let Some(entries) = entries {
                for entry in entries {
                    let path = entry.map_err(|e| e.to_string())?.path();
                    if path.extension().is_some_and(|e| e == "json") {
                        let completed: State = load(&path)?.ok_or("goal history disappeared")?;
                        if completed.wake_id == wake_id && completed.pending.is_none() {
                            state = completed;
                            break;
                        }
                    }
                }
            }
        }

        if state.wake_id != wake_id {
            let _permit = tokio::time::timeout(std::time::Duration::from_secs(30), self.running.acquire())
                .await
                .map_err(|_| "the sentinel home is busy")?
                .map_err(|e| e.to_string())?;
            let prepared = tokio::time::timeout(std::time::Duration::from_secs(60), async {
                let capabilities = self.runtime.capabilities(&registration, store.directory()).await?;
                let model = self.runtime.model(&registration).await?;
                Ok::<_, String>((capabilities, model))
            })
            .await
            .map_err(|_| "preparing the goal sentinel timed out")?;
            let result = match prepared {
                Ok((capabilities, model)) => {
                    crate::wake::wake(
                        &store,
                        state.clone(),
                        &wake_id,
                        WakeReason::Goal { review: Box::new(request.review.clone()) },
                        self.runtime.now(),
                        &crate::wake::Environment {
                            model: model.as_ref(),
                            look: capabilities.look.as_ref(),
                            web: capabilities.web.as_ref(),
                            machines: &capabilities.machines,
                            limits: &Limits::default(),
                        },
                    )
                    .await
                }
                Err(error) => Err(error),
            };
            match result {
                Ok(completed) => state = completed,
                Err(reason) => {
                    // Reload a completed snapshot before recording a failed attempt.
                    state = store.load()?.unwrap_or(state);
                    if state.wake_id != wake_id {
                        state.sequence += 1;
                        state.wake_id = wake_id;
                        state.pending = None;
                        state.completed_at = self.runtime.now();
                        state.conclusion = Conclusion::NotJudged { reason };
                        store.commit(&state)?;
                    }
                }
            }
        }
        let judgment = match state.conclusion {
            Conclusion::Goal { judgment } => judgment,
            Conclusion::NotJudged { reason } => Judgment::NotJudged { reason },
            _ => return Err("goal wake ended without a judgment".into()),
        };
        save(&receipt_path, &Receipt { input, judgment: Some(judgment.clone()) })?;
        let current = lifecycle::get(self.records.as_ref(), scope, &request.id).await.map_err(|e| e.to_string())?;
        if current.is_none_or(|(_, current)| current != registration) || !self.runtime.assigned(&registration).await? {
            return Err("goal sentinel assignment changed during its judgment".into());
        }
        Ok(judgment)
    }
}
