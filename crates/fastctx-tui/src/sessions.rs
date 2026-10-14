//! The World's tools in the sessions the user opens on this machine, as an
//! interface asks after them and changes them (`fastctx_install::apply`).
//! These read and write this machine's files, not the World: no node is
//! asked.

pub use fastctx_install::apply::{Host, Report, Standing, Status};

use fastctx_install::apply::{self, Hosts, Places};
use fastctx_install::codex::Approval;

/// Why the World's tools did not go in.
#[derive(Clone, Debug, thiserror::Error)]
pub enum Refused {
    /// Codex's output limit, shared by every tool, is set lower than the
    /// World's answers need, and raising it was not agreed to.
    #[error("Codex's tool_output_token_limit is {current}; the World's answers need {requested}")]
    OutputLimit { current: i64, requested: u64 },
    #[error("{0}")]
    Failed(String),
}

/// How each host here stands.
pub async fn status() -> Result<Status, String> {
    blocking(|| {
        let places = Places::here().map_err(|error| error.to_string())?;
        apply::status_checked(&places).map_err(|error| error.to_string())
    })
    .await
}

/// Applies or withdraws one home, with `take_back` rewriting what the user
/// changed of the World's tools; directory selection is independent of PATH.
pub async fn change_at(
    host: Host,
    home: std::path::PathBuf,
    into: bool,
    raise: bool,
    take_back: bool,
) -> Result<Report, Refused> {
    blocking(move || {
        let places = Places::here().map_err(|error| Refused::Failed(error.to_string()))?;
        let places = places.at_home(host, &home).map_err(|error| Refused::Failed(error.to_string()))?;
        let hosts = Hosts { codex: host == Host::Codex, claude: host == Host::Claude };
        let approval = if raise { Approval::RaiseOutputLimit } else { Approval::Require };
        let edits = if take_back { apply::Edits::TakeBack } else { apply::Edits::Keep };
        let result = if into {
            apply::apply_reporting(&places, hosts, approval, edits, None)
        } else {
            apply::unapply(&places, hosts)
        };
        result.map_err(|error| match error {
            apply::Error::Edit(fastctx_install::Error::TokenLimitApproval { current, requested, .. }) => {
                Refused::OutputLimit { current, requested }
            }
            error => Refused::Failed(error.to_string()),
        })
    })
    .await
}

/// Puts the World's tools into every host installed here; `raise` agrees
/// to raising Codex's shared output limit when it is set too low.
pub async fn apply(raise: bool) -> Result<Report, Refused> {
    blocking(move || {
        let places = Places::here().map_err(|error| Refused::Failed(error.to_string()))?;
        let approval = if raise { Approval::RaiseOutputLimit } else { Approval::Require };
        apply::apply(&places, Hosts::present(&places), approval).map_err(|error| match error {
            apply::Error::Edit(fastctx_install::Error::TokenLimitApproval { current, requested, .. }) => {
                Refused::OutputLimit { current, requested }
            }
            error => Refused::Failed(error.to_string()),
        })
    })
    .await
}

/// Takes the World's tools out of every host.
pub async fn unapply() -> Result<Report, String> {
    blocking(|| {
        let places = Places::here().map_err(|error| error.to_string())?;
        apply::unapply(&places, Hosts { codex: true, claude: true }).map_err(|error| error.to_string())
    })
    .await
}

async fn blocking<T: Send + 'static, E: Send + 'static + From<String>>(
    work: impl FnOnce() -> Result<T, E> + Send + 'static,
) -> Result<T, E> {
    tokio::task::spawn_blocking(work).await.map_err(|error| E::from(format!("the work stopped: {error}")))?
}

impl From<String> for Refused {
    fn from(text: String) -> Self {
        Self::Failed(text)
    }
}
