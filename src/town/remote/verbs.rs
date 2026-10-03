//! Calls other machines' towns make to this one (`super::calls` makes them): a resident's hands,
//! `fastctx w`, asking for a colleague, and the user talking to a resident who lives here. By
//! the time a call arrives here its machine has been verified and its grant checked
//! (`world::node::executor`).

use super::{Peer, valid_town};
use crate::hands::Starter;
use crate::hands::run::{self as hand_run, Runner};
use crate::hands::write::{self as hand_write, Writer};
use crate::model::ToolResponse;
use crate::session::SessionContext;
use crate::town::core::{Spoken, Town};
use crate::town::records::is_resident_id;
use crate::town::who::Query;
use crate::world::messages::Call;
use crate::world::node::executor::VerbOutcome;
use crate::write::{ApplyPatchRequest, EditRequest, WriteError, WriteRequest, WriteResult};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;

/// The resident a hand acts for, as its own machine names it: its id and the town it is in.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Guest {
    pub(crate) resident: String,
    #[serde(default)]
    pub(crate) town: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct RunCall {
    #[serde(flatten)]
    pub(crate) guest: Guest,
    pub(crate) command: String,
    #[serde(default)]
    pub(crate) cwd: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct WriteCall<T> {
    #[serde(flatten)]
    pub(crate) guest: Guest,
    #[serde(flatten)]
    pub(crate) request: T,
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct ColleagueCall {
    pub(crate) requester: String,
    #[serde(default)]
    pub(crate) town: Option<String>,
    pub(crate) prompt: String,
    pub(crate) label: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct SayCall {
    pub(crate) resident: String,
    pub(crate) text: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct OpenCall {
    pub(crate) agent: String,
    #[serde(default)]
    pub(crate) town: Option<String>,
    pub(crate) words: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct StopCall {
    pub(crate) resident: String,
}

/// Answers one town call from the machine `from`.
pub(crate) async fn answer(
    town: &Arc<Town>,
    from: &str,
    call: Call,
    session: &Arc<SessionContext>,
) -> VerbOutcome {
    dispatch(town, from, call, session)
        .await
        .unwrap_or_else(VerbOutcome::error_text)
}

async fn dispatch(
    town: &Arc<Town>,
    from: &str,
    call: Call,
    session: &Arc<SessionContext>,
) -> Result<VerbOutcome, String> {
    let args = call.args;
    match call.verb.as_str() {
        "hand_run" => hand_run_here(town, from, decode(args)?, session).await,
        "hand_edit" => {
            let call: WriteCall<EditRequest> = decode(args)?;
            let what = call.request.file_path.clone();
            write_here(town, from, call.guest, what, move |writer| {
                hand_write::edit(writer, call.request)
            })
            .await
        }
        "hand_write" => {
            let call: WriteCall<WriteRequest> = decode(args)?;
            let what = call.request.file_path.clone();
            write_here(town, from, call.guest, what, move |writer| {
                hand_write::write(writer, call.request)
            })
            .await
        }
        "hand_apply_patch" => {
            let call: WriteCall<ApplyPatchRequest> = decode(args)?;
            let what = hand_write::patch_files(&call.request.input);
            write_here(town, from, call.guest, what, move |writer| {
                hand_write::apply_patch(writer, call.request)
            })
            .await
        }
        "who" => {
            let query: Query = decode(args)?;
            let answer = crate::town::who::answer(town.world_dir(), town.machine(), &query);
            Ok(VerbOutcome {
                response: ToolResponse::text(String::new()),
                facts: fact("answer", json!(answer)),
            })
        }
        "colleague_ask" => {
            let call: ColleagueCall = decode(args)?;
            if !is_resident_id(&call.requester) {
                return Err(format!("{} is not a resident id.", call.requester));
            }
            if let Some(name) = &call.town
                && !valid_town(name)
            {
                return Err(format!("{name} is not a town's name."));
            }
            let requester = Peer::new(from, &call.requester);
            town.ask_colleague(&requester, call.town, &call.prompt, call.label.trim())
                .await
                .map(|asked| VerbOutcome {
                    response: ToolResponse::text(String::new()),
                    facts: fact("id", json!(asked.id)),
                })
        }
        "talk_say" => {
            let call: SayCall = decode(args)?;
            town.say_as_user(&call.resident, &call.text)
                .await
                .map(|(spoken, turn)| VerbOutcome {
                    response: ToolResponse::text(String::new()),
                    facts: BTreeMap::from([
                        ("landed".to_string(), json!(landed(spoken))),
                        ("turn".to_string(), json!(turn)),
                    ]),
                })
        }
        "talk_open" => {
            let call: OpenCall = decode(args)?;
            if let Some(name) = &call.town
                && !valid_town(name)
            {
                return Err(format!("{name} is not a town's name."));
            }
            town.open_resident(&call.agent, call.town, &call.words)
                .await
                .map(|resident| VerbOutcome {
                    response: ToolResponse::text(String::new()),
                    facts: fact("resident", json!(resident)),
                })
        }
        "talk_stop" => {
            let call: StopCall = decode(args)?;
            town.interrupt(&call.resident)
                .await
                .map(|stopped| VerbOutcome {
                    response: ToolResponse::text(String::new()),
                    facts: fact("stopped", json!(stopped)),
                })
        }
        other => Err(format!("This machine's town does not answer \"{other}\".")),
    }
}

/// How words put into a session landed, as a talk reports it.
pub(crate) fn landed(spoken: Spoken) -> &'static str {
    match spoken {
        Spoken::NewTurn => "started a new turn",
        Spoken::JoinedTurn => "joined its current turn",
        Spoken::Queued => "handed to its host",
    }
}

async fn hand_run_here(
    town: &Arc<Town>,
    from: &str,
    call: RunCall,
    session: &Arc<SessionContext>,
) -> Result<VerbOutcome, String> {
    let starter = starter(from, call.guest)?;
    let runner = Runner {
        town: Arc::clone(town),
        starter,
    };
    let result = hand_run::run(
        &runner,
        &session.command_environment,
        call.command.clone(),
        call.cwd,
    )
    .await;
    let (what, outcome) = hand_run::action(&call.command, &result);
    let response = match &result {
        Ok(result) => hand_run::response(result),
        Err(error) => ToolResponse::error(error.clone()),
    };
    Ok(VerbOutcome {
        response,
        facts: action_facts(what, outcome),
    })
}

async fn write_here(
    town: &Arc<Town>,
    from: &str,
    guest: Guest,
    what: String,
    operation: impl FnOnce(&Writer) -> Result<WriteResult, WriteError> + Send + 'static,
) -> Result<VerbOutcome, String> {
    let writer = Writer::new(Arc::clone(town), starter(from, guest)?);
    let outcome = tokio::task::spawn_blocking(move || operation(&writer))
        .await
        .map_err(|error| format!("The write stopped: {error}"))?;
    let result = hand_write::action(&outcome);
    let response = match outcome {
        Ok(written) => ToolResponse::text(written.text),
        Err(error) => ToolResponse::error(error.to_string()),
    };
    Ok(VerbOutcome {
        response,
        facts: action_facts(what, result),
    })
}

/// The resident a hand from `from` acts for here. Its id and town become directory names on
/// this machine, so they must be an id and a town's name, never a path.
fn starter(from: &str, guest: Guest) -> Result<Starter, String> {
    if !is_resident_id(&guest.resident) {
        return Err(format!("{} is not a resident id.", guest.resident));
    }
    if let Some(name) = &guest.town
        && !valid_town(name)
    {
        return Err(format!("{name} is not a town's name."));
    }
    Ok(Starter::Guest {
        peer: Peer::new(from, &guest.resident),
        town: guest.town,
    })
}

/// The two last fields of the action line the caller's machine records.
fn action_facts(what: String, result: String) -> BTreeMap<String, Value> {
    BTreeMap::from([
        ("what".to_string(), json!(what)),
        ("result".to_string(), json!(result)),
    ])
}

fn fact(name: &str, value: Value) -> BTreeMap<String, Value> {
    BTreeMap::from([(name.to_string(), value)])
}

fn decode<T: DeserializeOwned>(args: Value) -> Result<T, String> {
    serde_json::from_value(args).map_err(|error| format!("Unreadable call arguments: {error}"))
}
