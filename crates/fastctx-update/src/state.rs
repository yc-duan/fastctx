//! One calculation of a device's offer, policy and current update stage.

use fastctx_types::renewal::Phase;
use fastctx_types::update::{Channel, FailureReason, Kind, Manifest, Network, Problem, Stage, State, newest, release};

pub struct Context {
    pub kind: Kind,
    pub connected: bool,
    pub hub_here: bool,
    pub networks: Vec<Network>,
    pub cached: Vec<Manifest>,
    pub activity: Option<Stage>,
}

pub fn merge(networks: impl IntoIterator<Item = Network>) -> Option<Network> {
    let mut merged: Option<Network> = None;
    for network in networks {
        let Some(kept) = &mut merged else {
            merged = Some(network);
            continue;
        };
        kept.stable = newest(kept.stable.as_ref(), network.stable.as_ref()).cloned();
        kept.beta = newest(kept.beta.as_ref(), network.beta.as_ref()).cloned();
        if network.policy.minimum.as_deref().and_then(release) > kept.policy.minimum.as_deref().and_then(release) {
            kept.policy.minimum = network.policy.minimum;
        }
        kept.policy.managed_auto &= network.policy.managed_auto;
        kept.hub = match (kept.hub.take(), network.hub) {
            (Some(a), Some(b)) => Some(if release(&a) < release(&b) { a } else { b }),
            (a, b) => a.or(b),
        };
        kept.machines.extend(network.machines);
        kept.due.extend(network.due);
    }
    merged
}

/// Completes raw local facts using exactly the same rules on computers and phones.
pub fn calculate(mut state: State, context: Context) -> State {
    let kernel = state.running.as_deref().unwrap_or(env!("CARGO_PKG_VERSION"));
    let running = state.app.as_ref().map_or(kernel, |app| app.version.as_str());
    state.channel = Channel::of(running);
    state.hub_major = if context.hub_here || !context.connected {
        None
    } else {
        context
            .networks
            .iter()
            .filter_map(|network| network.hub.as_deref().and_then(release).or_else(|| release(kernel)).map(|v| v.major))
            .min()
            .or_else(|| release(kernel).map(|v| v.major))
    };
    state.network = merge(context.networks);
    state.auto = state.settings.auto_for(context.kind, state.network.as_ref().map(|n| &n.policy));
    state.chosen = state.chosen.filter(|m| m.newer_than(running));
    state.offer = state.chosen.clone().or_else(|| {
        [state.looked.on(state.channel), state.network.as_ref().and_then(|n| n.on(state.channel))]
            .into_iter()
            .chain(context.cached.iter().filter(|m| state.channel.takes(&m.version)).map(Some))
            .fold(None, newest)
            .cloned()
    });
    state.progress = true;
    state.stage = stage(&state, context.activity);
    state
}

fn stage(state: &State, activity: Option<Stage>) -> Stage {
    if let Some(failed) = &state.failed {
        return Stage::Failed {
            version: failed.version.clone(),
            problem: Problem { reason: failed.reason.clone().unwrap_or(FailureReason::Other), why: failed.why.clone() },
        };
    }
    if let Some(renewing) = &state.renewing {
        let version = renewing.version.clone();
        return match renewing.phase {
            Phase::WaitingForIdle => Stage::WaitingIdle { version },
            Phase::AwaitingAdministrator => Stage::Administrator { version },
            Phase::Failed => Stage::Failed {
                version,
                problem: Problem { reason: FailureReason::Other, why: renewing.error.clone().unwrap_or_default() },
            },
            _ => Stage::Renewing { version, phase: renewing.phase.clone() },
        };
    }
    if let Some(activity) = activity {
        return activity;
    }
    if let Some(fetching) = &state.fetching {
        return Stage::Fetching {
            version: fetching.version.clone(),
            via: (fetching.received > 0).then_some(fetching.via),
            received: fetching.received,
            total: fetching.total,
        };
    }
    if let Some(version) = &state.take {
        return if !state.open && !cfg!(windows) {
            Stage::WaitingApp { version: version.clone() }
        } else {
            Stage::WaitingIdle { version: version.clone() }
        };
    }
    if let Some(offer) = &state.offer {
        let target = state.app.as_ref().map_or(state.kernel_target.as_str(), |a| a.target.as_str());
        if state.kept.iter().any(|k| k.version == offer.version && k.target == target) {
            return Stage::Ready { version: offer.version.clone() };
        }
    }
    Stage::Idle
}

pub fn automatic(state: &State) -> Option<String> {
    let running = state.app.as_ref().map(|a| a.version.as_str()).or(state.running.as_deref()).and_then(release);
    let fits = |version: &str| {
        let candidate = release(version);
        state.channel.takes(version)
            && candidate > running
            && state.hub_major.is_none_or(|major| candidate.is_some_and(|v| v.major <= major))
    };
    state
        .offer
        .as_ref()
        .map(|m| &m.version)
        .filter(|v| fits(v))
        .cloned()
        .or_else(|| state.looked.newer.iter().map(|m| &m.version).find(|v| fits(v)).cloned())
}
