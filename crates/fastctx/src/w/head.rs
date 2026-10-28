//! The head line of `w`, as long on a network of thousands as on one of
//! three (`world-town.md`; PINNED「工具为 AI 设计」): the machines that matter
//! here by name, every other only counted.
//!
//! The machines that matter: this one, those the conversation involves, the
//! project's, and those with its work on them now. Online, those come first,
//! then others by name, a dozen in all at most; offline and silent, only those
//! by name, unless the others are a few. A machine is said once: one offline
//! did not answer for being so.

use std::collections::BTreeSet;

use super::*;

/// How many machines online the head line names, unless more matter.
const NAMED_ONLINE: usize = 12;
/// How many others offline, or others silent, are named rather than counted.
const NAMED_OTHERS: usize = 3;

pub(super) fn line(view: &TownView, now: Timestamp, local_only: bool) -> String {
    let project: BTreeSet<&MachineName> = match &view.scope {
        Scope::Town { record: Some(record), .. } => record.machines.iter().collect(),
        _ => BTreeSet::new(),
    };
    let working: BTreeSet<&MachineName> = view
        .residents
        .iter()
        .filter(|resident| resident.here)
        .map(|resident| &resident.about.machine)
        .chain(view.jobs.iter().map(|job| &job.machine))
        .collect();
    let matters = |machine: &MachineName| {
        *machine == view.machine
            || view.involved.contains(machine)
            || project.contains(machine)
            || working.contains(machine)
    };
    let mut parts = Vec::new();

    let mut online: Vec<_> = view
        .machines
        .iter()
        .filter_map(|state| match state {
            MachineState::Answered(machine) => Some(machine),
            _ => None,
        })
        .collect();
    online.sort_by_key(|machine| (!matters(&machine.name), machine.to_string()));
    let named = online.iter().filter(|machine| matters(&machine.name)).count().max(NAMED_ONLINE.min(online.len()));
    if !online.is_empty() {
        let names = online[..named].iter().map(ToString::to_string).collect::<Vec<_>>().join(", ");
        let more = online.len() - named;
        let label = if local_only { "local records" } else { "online" };
        parts.push(if more > 0 { format!("{label}: {names} and {more} more") } else { format!("{label}: {names}") });
    }

    let few_offline = view
        .machines
        .iter()
        .filter(|state| matches!(state, MachineState::Offline { machine, .. } if !matters(&machine.name)))
        .count()
        <= NAMED_OTHERS;
    let mut offline = Vec::new();
    let (mut others, mut longest) = (0, None::<Timestamp>);
    for state in &view.machines {
        if let MachineState::Offline { machine, since } = state {
            if few_offline || matters(&machine.name) {
                offline.push(match since {
                    Some(since) => format!("{machine} for {}", ago(*since, now)),
                    None => machine.to_string(),
                });
            } else {
                others += 1;
                longest = match (longest, since) {
                    (Some(earliest), Some(since)) => Some(earliest.min(*since)),
                    (None, since) => *since,
                    (earliest, None) => earliest,
                };
            }
        }
    }
    let rest = |count: usize, named: bool, said: &str| {
        let whom = if named {
            "others"
        } else if count == 1 {
            "machine"
        } else {
            "machines"
        };
        format!("{count} {whom} {said}")
    };
    if !offline.is_empty() || others > 0 {
        let mut said = offline.join(", ");
        if others > 0 {
            let mut counted = rest(others, !offline.is_empty(), "offline");
            if let Some(longest) = longest {
                counted.push_str(&format!(", the longest for {}", ago(longest, now)));
            }
            said = if said.is_empty() { counted } else { format!("{said}; {counted}") };
        }
        parts.push(format!("offline: {said}"));
    }

    // Those that did not answer for one reason are said together: a hub
    // this computer cannot reach leaves every machine on it unasked.
    let few_silent = view
        .machines
        .iter()
        .filter(|state| matches!(state, MachineState::NoAnswer { machine, .. } if !matters(&machine.name)))
        .count()
        <= NAMED_OTHERS;
    let mut silent: Vec<(&str, Vec<String>)> = Vec::new();
    let mut unnamed = 0;
    for state in &view.machines {
        if let MachineState::NoAnswer { machine, reason } = state {
            if !few_silent && !matters(&machine.name) {
                unnamed += 1;
                continue;
            }
            match silent.iter_mut().find(|(said, _)| *said == reason.as_str()) {
                Some((_, names)) => names.push(machine.to_string()),
                None => silent.push((reason, vec![machine.to_string()])),
            }
        }
    }
    if !silent.is_empty() || unnamed > 0 {
        let mut said = silent
            .iter()
            .map(|(reason, names)| format!("{} ({reason})", names.join(", ")))
            .collect::<Vec<_>>()
            .join(", ");
        if unnamed > 0 {
            let counted = rest(unnamed, !said.is_empty(), "did not answer");
            said = if said.is_empty() { counted } else { format!("{said}; {counted}") };
        }
        parts.push(if silent.is_empty() { said } else { format!("did not answer: {said}") });
    }

    let place = match &view.scope {
        Scope::Town { town, .. } => town.as_str(),
        Scope::World { .. } => "all projects",
    };
    format!("=== w: {place} ({}) ===\n", parts.join("; "))
}
