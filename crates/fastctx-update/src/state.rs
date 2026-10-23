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
        kept.repairs.extend(network.repairs);
        kept.repair_supported &= network.repair_supported;
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
    let target = state.app.as_ref().map_or(state.kernel_target.as_str(), |app| app.target.as_str());
    let candidates: Vec<_> = state
        .offer
        .iter()
        .chain(state.looked.newer.iter())
        .chain(context.cached.iter())
        .chain(state.looked.stable.iter())
        .chain(state.looked.beta.iter())
        .chain(state.network.iter().flat_map(|n| n.stable.iter().chain(n.beta.iter())))
        .collect();
    state.available = state
        .chosen
        .as_ref()
        .filter(|m| m.package(target).is_some())
        .cloned()
        .or_else(|| latest_for(candidates.iter().copied(), running, target, None).cloned());
    state.automatic = latest_for(state.chosen.iter(), running, target, state.hub_major)
        .or_else(|| latest_for(candidates.iter().copied(), running, target, state.hub_major))
        .map(|m| m.version.clone());
    state.above_hub = state
        .available
        .as_ref()
        .and_then(|m| m.semver())
        .is_some_and(|v| state.hub_major.is_some_and(|major| v.major > major));
    if !fastctx_platform::follows_releases() {
        state.auto = false;
        state.available = None;
        state.automatic = None;
        state.above_hub = false;
        state.offer = None;
        state.chosen = None;
        state.take = None;
        state.looked = Default::default();
        state.network = None;
    }
    state.progress = true;
    state.preferences = true;
    state.stage = stage(&state, context.activity, running);
    state
}

fn stage(state: &State, activity: Option<Stage>, running: &str) -> Stage {
    if let Some(repair) = &state.repair
        && (repair.finished.is_none() || release(&repair.version) >= release(running)) {
        if repair.finished.is_none() {
            return Stage::Repairing { version: repair.version.clone(), layer: repair.steps.last().map(|s| s.layer.clone()).unwrap_or_else(|| "package".into()) };
        }
        if !repair.success {
            return Stage::Failed { version: repair.version.clone(), problem: Problem::from(repair.error.clone().unwrap_or_else(|| "repair did not complete".into())) };
        }
    }
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
            // An earlier release's wait for an administrator goes on by
            // itself now, as the boot service's step.
            Phase::AwaitingAdministrator => Stage::Renewing { version, phase: Phase::UpdatingGuardian },
            Phase::Failed => Stage::Failed {
                version,
                problem: Problem { reason: FailureReason::Other, why: renewing.error.clone().unwrap_or_default() },
            },
            _ => Stage::Renewing { version, phase: renewing.phase.clone() },
        };
    }
    if let Some(fetching) = &state.fetching {
        return Stage::Fetching {
            version: fetching.version.clone(),
            via: (fetching.received > 0).then_some(fetching.via),
            received: fetching.received,
            total: fetching.total,
        };
    }
    if let Some(activity) = activity {
        return activity;
    }
    if let Some(version) = &state.take {
        return if !state.open && !cfg!(windows) {
            Stage::WaitingApp { version: version.clone() }
        } else {
            Stage::WaitingIdle { version: version.clone() }
        };
    }
    // A package kept for others of the version running here is no update
    // waiting to be installed.
    if let Some(offer) = state.available.as_ref().or(state.offer.as_ref()).filter(|m| m.newer_than(running)) {
        let target = state.app.as_ref().map_or(state.kernel_target.as_str(), |a| a.target.as_str());
        if state.kept.iter().any(|k| k.version == offer.version && k.target == target) {
            return Stage::Ready { version: offer.version.clone() };
        }
    }
    Stage::Idle
}

/// The newest newer release on the running line that has the device's package.
pub fn latest_for<'a>(
    manifests: impl IntoIterator<Item = &'a Manifest>,
    running: &str,
    target: &str,
    major: Option<u64>,
) -> Option<&'a Manifest> {
    let channel = Channel::of(running);
    manifests
        .into_iter()
        .filter(|m| {
            channel.takes(&m.version)
                && m.newer_than(running)
                && m.package(target).is_some()
                && major.is_none_or(|major| m.semver().is_some_and(|v| v.major <= major))
        })
        .max_by_key(|m| m.semver())
}

pub fn automatic(state: &State) -> Option<String> {
    state.automatic.clone()
}
