//! The hub link: which path to dial (`direct`, `system`, or `auto`), and the dialed
//! connection handed to the session for the application handshake.

pub(crate) mod dns;
pub(crate) mod netpath;
pub(crate) mod proxy;
pub(crate) mod tls;
pub(crate) mod ws;

use crate::world::NetworkMode;
use std::time::Duration;
pub(crate) use tls::{Learned, Verify};
pub(crate) use ws::{Dialed, Endpoint, Path};

/// How often `auto` re-probes the direct path while running on the system path.
pub(crate) const DIRECT_REPROBE_INTERVAL: Duration = Duration::from_secs(600);

/// What the link layer needs to dial.
#[derive(Clone, Debug)]
pub(crate) struct DialPlan {
    pub(crate) endpoints: Vec<Endpoint>,
    pub(crate) mode: NetworkMode,
    /// The physical interface `direct` should pin to; `None` means the best candidate.
    pub(crate) interface: Option<String>,
    /// Which mode last succeeded, so `auto` tries it first.
    pub(crate) preferred: Option<NetworkMode>,
}

/// Why every path failed, kept per path so status can explain both.
#[derive(Clone, Debug, Default)]
pub(crate) struct DialFailure {
    pub(crate) direct: Option<String>,
    pub(crate) system: Option<String>,
}

impl std::fmt::Display for DialFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut parts = Vec::new();
        if let Some(direct) = &self.direct {
            parts.push(format!("direct: {direct}"));
        }
        if let Some(system) = &self.system {
            parts.push(format!("system: {system}"));
        }
        if parts.is_empty() {
            formatter.write_str("no path was tried")
        } else {
            formatter.write_str(&parts.join("; "))
        }
    }
}

/// Dials the hub over the first path that works, in the order the plan prescribes.
pub(crate) async fn dial(plan: &DialPlan, verify: &Verify) -> Result<Dialed, DialFailure> {
    let order: Vec<NetworkMode> = match plan.mode {
        NetworkMode::Direct => vec![NetworkMode::Direct],
        NetworkMode::System => vec![NetworkMode::System],
        NetworkMode::Auto => match plan.preferred {
            Some(NetworkMode::System) => vec![NetworkMode::System, NetworkMode::Direct],
            _ => vec![NetworkMode::Direct, NetworkMode::System],
        },
    };
    let mut failure = DialFailure::default();
    for mode in order {
        match mode {
            NetworkMode::Direct => match dial_direct(plan, verify).await {
                Ok(dialed) => return Ok(dialed),
                Err(error) => failure.direct = Some(error),
            },
            NetworkMode::System => match dial_system(plan, verify).await {
                Ok(dialed) => return Ok(dialed),
                Err(error) => failure.system = Some(error),
            },
            NetworkMode::Auto => unreachable!("auto expands to concrete modes"),
        }
    }
    Err(failure)
}

/// Dials only the direct path; used by `auto`'s background re-probe.
pub(crate) async fn dial_direct(plan: &DialPlan, verify: &Verify) -> Result<Dialed, String> {
    let view = netpath::scan()?;
    let interface = view.choose_physical(plan.interface.as_deref())?.clone();
    let mut last_error = String::new();
    for endpoint in &plan.endpoints {
        match ws::dial_direct(endpoint, verify, &view, &interface).await {
            Ok(dialed) => return Ok(dialed),
            Err(error) => last_error = format!("{endpoint}: {error}"),
        }
    }
    Err(last_error)
}

async fn dial_system(plan: &DialPlan, verify: &Verify) -> Result<Dialed, String> {
    let mut last_error = String::new();
    for endpoint in &plan.endpoints {
        let proxy = proxy::discover(&endpoint.host);
        match ws::dial_system(endpoint, verify, proxy.as_ref()).await {
            Ok(dialed) => return Ok(dialed),
            Err(error) => last_error = format!("{endpoint}: {error}"),
        }
    }
    Err(last_error)
}
