//! Request observations survive a node restart until their turn end is durable.

use std::collections::BTreeMap;
use std::io;

use fastctx_types::layout::ConversationDir;
use fastctx_types::session::{Happened, RequestUsage, SessionChange, TurnEffort, TurnFacts, TurnTokens};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Default, Serialize, Deserialize)]
struct Observations {
    efforts: BTreeMap<u64, Value>,
    responses: BTreeMap<u64, RequestUsage>,
}

fn path(directory: &ConversationDir, turn: &str) -> std::path::PathBuf {
    directory.turn_facts().join(format!("{}.json", crate::speech::fixed_id("turn", &[turn])))
}

fn read(path: &std::path::Path) -> io::Result<Observations> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(io::Error::other),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Observations::default()),
        Err(error) => Err(error),
    }
}

pub fn observe(directory: &ConversationDir, happened: &Happened) -> io::Result<()> {
    let Happened::Changed { turn: Some(turn), change } = happened else { return Ok(()) };
    if !matches!(change, SessionChange::ModelRequest { .. } | SessionChange::ModelResponse { usage: Some(_), .. })
        || crate::delivery::terminal(directory, turn)?.is_some()
    {
        return Ok(());
    }
    let _transaction = crate::resident_storage::lock();
    let path = path(directory, turn);
    let mut observations = read(&path)?;
    match change {
        SessionChange::ModelRequest { step, effort, .. } => {
            if !effort.is_null() {
                observations.efforts.insert(*step, effort.clone());
            }
        }
        SessionChange::ModelResponse { step, usage: Some(usage) } => {
            // The final result can correct a stop chunk's usage for the same request.
            observations.responses.insert(*step, usage.clone());
        }
        _ => unreachable!(),
    }
    fastctx_platform::ensure_private_dir(path.parent().expect("facts directory"))?;
    fastctx_platform::publish(
        &path,
        &serde_json::to_vec(&observations).map_err(io::Error::other)?,
        fastctx_platform::Permissions::Private,
    )
}

pub fn finish(directory: &ConversationDir, turn: &str) -> io::Result<Option<TurnFacts>> {
    let _transaction = crate::resident_storage::lock();
    let observations = read(&path(directory, turn))?;
    let mut facts = TurnFacts::default();
    for effort in observations.efforts.into_values() {
        facts.effort(TurnEffort::Requested { value: effort });
    }
    let (mut input_tokens, mut output_tokens, mut cache_read_input_tokens, mut cache_creation_input_tokens) =
        (0_u64, 0_u64, 0_u64, 0_u64);
    let has_usage = !observations.responses.is_empty();
    let add = |a: u64, b: u64| a.checked_add(b).ok_or_else(|| io::Error::other("Claude turn token count overflow"));
    for usage in observations.responses.into_values() {
        facts.model(&usage.model);
        input_tokens = add(input_tokens, usage.input_tokens)?;
        output_tokens = add(output_tokens, usage.output_tokens)?;
        cache_read_input_tokens = add(cache_read_input_tokens, usage.cache_read_input_tokens)?;
        cache_creation_input_tokens = add(cache_creation_input_tokens, usage.cache_creation_input_tokens)?;
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
}

pub fn settled(directory: &ConversationDir, turn: &str) -> io::Result<()> {
    let _transaction = crate::resident_storage::lock();
    match std::fs::remove_file(path(directory, turn)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}
