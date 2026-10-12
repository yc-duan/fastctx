//! Machine and server names as the person types them, checked by the World's
//! one rule (`fastctx_types::names`, `world-network.md` 机器的名字) before
//! anything is sent, so the page can say what is wrong in its own words.

use fastctx_types::{HubName, MachineName, NameProblem};
use serde::Deserialize;

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum NameKind {
    Machine,
    Hub,
}

/// What is wrong with `text` as a name of `kind`; nothing when it is one.
#[fastctx_gui_command::command]
pub(crate) async fn name_problem(kind: NameKind, text: String) -> Result<Option<NameProblem>, String> {
    let checked = match kind {
        NameKind::Machine => MachineName::try_from(text).map(drop),
        NameKind::Hub => HubName::try_from(text).map(drop),
    };
    Ok(checked.err().map(|error| error.problem.expect("a machine or hub name says what is wrong with it")))
}
