//! Generate the GUI's frozen JSON contract samples from actual Rust values.

use std::io::{self, Write};

use serde::Serialize;

#[path = "gui_wire_fixtures/agents.rs"]
mod agents;
#[path = "gui_wire_fixtures/session.rs"]
mod session;
#[path = "gui_wire_fixtures/world.rs"]
mod world;

#[derive(Serialize)]
struct Fixtures {
    session: session::Fixtures,
    world: world::Fixtures,
    agents: agents::Fixtures,
}

fn main() {
    let fixtures = Fixtures { session: session::fixtures(), world: world::fixtures(), agents: agents::fixtures() };
    let mut out = io::stdout().lock();
    serde_json::to_writer_pretty(&mut out, &fixtures).expect("serialize GUI fixtures");
    writeln!(out).expect("write GUI fixtures");
}
