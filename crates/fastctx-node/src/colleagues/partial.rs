//! Keep known agents while a replacement branch is still being read.

use fastctx_types::collaboration::{Branch, NativeAgent};
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn retain_agents(branch: &mut Branch, previous: &Branch) {
    if !branch.reading {
        return;
    }
    fn ids(agents: &[NativeAgent], known: &mut BTreeSet<String>) {
        for agent in agents {
            known.insert(agent.id.clone());
            ids(&agent.children, known);
        }
    }
    fn index<'a>(agents: &'a [NativeAgent], by_id: &mut BTreeMap<&'a str, &'a NativeAgent>) {
        for agent in agents {
            by_id.insert(&agent.id, agent);
            index(&agent.children, by_id);
        }
    }
    fn retain(
        agents: &mut Vec<NativeAgent>,
        previous: &[NativeAgent],
        known: &BTreeSet<String>,
        by_id: &BTreeMap<&str, &NativeAgent>,
    ) {
        for agent in &mut *agents {
            if let Some(old) = by_id.get(agent.id.as_str()) {
                retain(&mut agent.children, &old.children, known, by_id);
            }
        }
        for old in previous {
            if !known.contains(&old.id) {
                let mut agent = old.clone();
                agent.reading = true;
                agent.children.clear();
                retain(&mut agent.children, &old.children, known, by_id);
                agents.push(agent);
            }
        }
    }
    let mut known = BTreeSet::new();
    ids(&branch.agents, &mut known);
    let mut by_id = BTreeMap::new();
    index(&previous.agents, &mut by_id);
    retain(&mut branch.agents, &previous.agents, &known, &by_id);
}
