//! Request observations survive a node restart until their turn end is durable.
mod storage;
pub(crate) use storage::collect;

use fastctx_types::layout::ConversationDir;
use fastctx_types::session::{Happened, RequestUsage, SessionChange, SessionEvent, TurnEffort, TurnFacts, TurnTokens};
use serde_json::Value;
use std::{collections::HashSet, io};

pub async fn observe(directory: &ConversationDir, event: SessionEvent) -> io::Result<SessionEvent> {
    if !matches!(&event.happened,Happened::Changed { turn:Some(_),change:SessionChange::ModelRequest { effort,.. }} if !effort.is_null())
        && !matches!(
            &event.happened,
            Happened::Changed { turn: Some(_), change: SessionChange::ModelResponse { usage: Some(_), .. } }
        )
    {
        return Ok(event);
    }
    let directory = directory.clone();
    tokio::task::spawn_blocking(move || {
        let Happened::Changed { turn: Some(turn), change } = &event.happened else { unreachable!() };
        let (kind, step, bytes) = match change {
            SessionChange::ModelRequest { step, effort, .. } => ("effort", *step, serde_json::to_vec(effort)?),
            SessionChange::ModelResponse { step, usage: Some(usage) } => {
                ("response", *step, serde_json::to_vec(usage)?)
            }
            _ => unreachable!(),
        };
        storage::put(&directory, turn, kind, step, bytes)?;
        Ok(event)
    })
    .await
    .map_err(io::Error::other)?
}

pub async fn finish(directory: &ConversationDir, turn: &str) -> io::Result<Option<TurnFacts>> {
    let (directory, turn) = (directory.clone(), turn.to_string());
    tokio::task::spawn_blocking(move || {
        storage::read(&directory, &turn, |rows, family| {
            let mut facts = TurnFacts::default();
            let mut after = String::new();
            let (mut efforts, mut models) = (HashSet::new(), HashSet::new());
            let (mut input_tokens, mut output_tokens, mut cache_read_input_tokens, mut cache_creation_input_tokens) =
                (0u64, 0u64, 0u64, 0u64);
            let mut has_usage = false;
            let add = |a: u64, b: u64| {
                a.checked_add(b).ok_or_else(|| fastctx_storage::Error::new("Claude turn token count overflow"))
            };
            loop {
                let page = rows.key_page::<Value>(family, &after, 128)?;
                let Some(last) = page.last() else {
                    break;
                };
                after = last.0.clone();
                for (key, value) in page {
                    if key.starts_with("effort-") {
                        if efforts.insert(value.clone()) {
                            facts.efforts.push(TurnEffort::Requested { value });
                        }
                    } else if key.starts_with("response-") {
                        let usage: RequestUsage = serde_json::from_value(value).map_err(|error| {
                            fastctx_storage::Error::with_kind(fastctx_storage::ErrorKind::Corrupt, error.to_string())
                        })?;
                        if models.insert(usage.model.clone()) {
                            facts.models.push(usage.model);
                        }
                        input_tokens = add(input_tokens, usage.input_tokens)?;
                        output_tokens = add(output_tokens, usage.output_tokens)?;
                        cache_read_input_tokens = add(cache_read_input_tokens, usage.cache_read_input_tokens)?;
                        cache_creation_input_tokens =
                            add(cache_creation_input_tokens, usage.cache_creation_input_tokens)?;
                        has_usage = true;
                    } else {
                        return Err(fastctx_storage::Error::with_kind(
                            fastctx_storage::ErrorKind::Corrupt,
                            "unrecognized Claude request observation",
                        ));
                    }
                }
            }
            if has_usage {
                facts.tokens = Some(TurnTokens::Claude {
                    input_tokens,
                    output_tokens,
                    cache_read_input_tokens,
                    cache_creation_input_tokens,
                });
            }
            Ok((!facts.is_empty()).then_some(facts))
        })
        .map(Option::flatten)
    })
    .await
    .map_err(io::Error::other)?
}

pub async fn settled(directory: &ConversationDir, turn: &str) -> io::Result<()> {
    let (directory, turn) = (directory.clone(), turn.to_string());
    tokio::task::spawn_blocking(move || storage::close(&directory, &turn)).await.map_err(io::Error::other)?
}
