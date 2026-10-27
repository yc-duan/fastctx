//! Generate the GUI's frozen JSON contract samples from actual Rust values.

use std::io::{self, Write};

use serde::Serialize;

#[path = "gui_wire_fixtures/accounts.rs"]
mod accounts;
#[path = "gui_wire_fixtures/agents.rs"]
mod agents;
#[path = "gui_wire_fixtures/attachments.rs"]
mod attachments;
#[path = "gui_wire_fixtures/claude.rs"]
mod claude;
#[path = "gui_wire_fixtures/composer.rs"]
mod composer;
#[path = "gui_wire_fixtures/composer_sync.rs"]
mod composer_sync;
#[path = "gui_wire_fixtures/control.rs"]
mod control;
#[path = "gui_wire_fixtures/file_read.rs"]
mod file_read;
#[path = "gui_wire_fixtures/file_search.rs"]
mod file_search;
#[path = "gui_wire_fixtures/focus.rs"]
mod focus;
#[path = "gui_wire_fixtures/logins.rs"]
mod logins;
#[path = "gui_wire_fixtures/machine_log.rs"]
mod machine_log;
#[path = "gui_wire_fixtures/pages.rs"]
mod pages;
#[path = "gui_wire_fixtures/profile.rs"]
mod profile;
#[path = "gui_wire_fixtures/renewal.rs"]
mod renewal;
#[path = "gui_wire_fixtures/session.rs"]
mod session;
#[path = "gui_wire_fixtures/since.rs"]
mod since;
#[path = "gui_wire_fixtures/stats.rs"]
mod stats;
#[path = "gui_wire_fixtures/updates.rs"]
mod updates;
#[path = "gui_wire_fixtures/usage.rs"]
mod usage;
#[path = "gui_wire_fixtures/usage_calls.rs"]
mod usage_calls;
#[path = "gui_wire_fixtures/world.rs"]
mod world;

#[path = "gui_wire_fixtures/management.rs"]
mod management;

#[path = "gui_wire_fixtures/run_facts.rs"]
mod run_facts;

#[derive(Serialize)]
struct Fixtures {
    updates: updates::Fixtures,
    profile: profile::Fixtures,
    machine_log: Vec<fastctx_types::machine_log::Page>,
    run_facts: run_facts::Fixtures,
    accounts: accounts::Fixtures,
    pages: pages::Fixtures,
    session: session::Fixtures,
    world: world::Fixtures,
    since: since::Fixtures,
    agents: agents::Fixtures,
    logins: logins::Fixtures,
    control: control::Fixtures,
    file_read: file_read::Fixtures,
    file_search: file_search::Fixtures,
    focus: focus::Fixtures,
    attachments: attachments::Fixtures,
    composer: composer::Fixtures,
    composer_sync: composer_sync::Fixtures,
    claude: claude::Fixtures,
    management: management::Fixtures,
    renewal: renewal::Fixtures,
    stats: stats::Fixtures,
    usage: usage::Fixtures,
    usage_calls: usage::Fixtures,
}

fn main() {
    let fixtures = Fixtures {
        updates: updates::fixtures(),
        profile: profile::fixtures(),
        machine_log: machine_log::fixtures(),
        run_facts: run_facts::fixtures(),
        accounts: accounts::fixtures(),
        pages: pages::fixtures(),
        session: session::fixtures(),
        world: world::fixtures(),
        since: since::fixtures(),
        agents: agents::fixtures(),
        logins: logins::fixtures(),
        control: control::fixtures(),
        file_read: file_read::fixtures(),
        file_search: file_search::fixtures(),
        focus: focus::fixtures(),
        attachments: attachments::fixtures(),
        composer: composer::fixtures(),
        composer_sync: composer_sync::fixtures(),
        claude: claude::fixtures(),
        management: management::fixtures(),
        renewal: renewal::fixtures(),
        stats: stats::fixtures(),
        usage: usage::fixtures(),
        usage_calls: usage_calls::fixtures(),
    };
    let mut out = io::stdout().lock();
    serde_json::to_writer_pretty(&mut out, &fixtures).expect("serialize GUI fixtures");
    writeln!(out).expect("write GUI fixtures");
}
