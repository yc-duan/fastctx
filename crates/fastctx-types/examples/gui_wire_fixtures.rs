//! Generate the GUI's frozen JSON contract samples from actual Rust values.

use std::io::{self, Write};

use serde::Serialize;

#[path = "gui_wire_fixtures/agents.rs"]
mod agents;
#[path = "gui_wire_fixtures/attachments.rs"]
mod attachments;
#[path = "gui_wire_fixtures/claude.rs"]
mod claude;
#[path = "gui_wire_fixtures/control.rs"]
mod control;
#[path = "gui_wire_fixtures/file_read.rs"]
mod file_read;
#[path = "gui_wire_fixtures/renewal.rs"]
mod renewal;
#[path = "gui_wire_fixtures/session.rs"]
mod session;
#[path = "gui_wire_fixtures/since.rs"]
mod since;
#[path = "gui_wire_fixtures/world.rs"]
mod world;

#[derive(Serialize)]
struct Fixtures {
    session: session::Fixtures,
    world: world::Fixtures,
    since: since::Fixtures,
    agents: agents::Fixtures,
    control: control::Fixtures,
    file_read: file_read::Fixtures,
    attachments: attachments::Fixtures,
    claude: claude::Fixtures,
    renewal: renewal::Fixtures,
}

fn main() {
    let fixtures = Fixtures {
        session: session::fixtures(),
        world: world::fixtures(),
        since: since::fixtures(),
        agents: agents::fixtures(),
        control: control::fixtures(),
        file_read: file_read::fixtures(),
        attachments: attachments::fixtures(),
        claude: claude::fixtures(),
        renewal: renewal::fixtures(),
    };
    let mut out = io::stdout().lock();
    serde_json::to_writer_pretty(&mut out, &fixtures).expect("serialize GUI fixtures");
    writeln!(out).expect("write GUI fixtures");
}
