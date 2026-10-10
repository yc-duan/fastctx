//! Cache cleanup follows the public withdrawal for each Claude home, including
//! homes whose receipt was already retired before an interrupted removal.

use crate::{Action, Request, Step};
use fastctx_install::apply::Host;
use fastctx_types::uninstall::Item;
use std::path::{Path, PathBuf};

pub(crate) fn directory(home: &Path) -> PathBuf {
    home.join("plugins/cache/fastctx")
}

pub(crate) fn include_steps(request: &mut Request) {
    let mut homes = request
        .steps
        .iter()
        .filter_map(|step| if let Action::ClaudeCache(home) = &step.action { Some(home.clone()) } else { None })
        .collect::<Vec<_>>();
    let mut additions = Vec::new();
    for (index, step) in request.steps.iter().enumerate() {
        if let Action::Host(integration) = &step.action
            && integration.host == Host::Claude
            && !homes.contains(&integration.home)
        {
            let item = Item {
                id: format!("claude-cache-{}", step.item.id),
                kind: "claude-cache".into(),
                what: "Delete the World's Claude plugin cache".into(),
                location: Some(directory(&integration.home).display().to_string()),
                admin: false,
            };
            homes.push(integration.home.clone());
            additions.push((
                index + 1,
                step.item.id.clone(),
                Step { item, action: Action::ClaudeCache(integration.home.clone()) },
            ));
        }
    }
    for (index, host_id, step) in additions.into_iter().rev() {
        let plan_index = request.plan.steps.iter().position(|item| item.id == host_id).expect("planned host step");
        request.plan.steps.insert(plan_index + 1, step.item.clone());
        request.steps.insert(index, step);
    }
}
