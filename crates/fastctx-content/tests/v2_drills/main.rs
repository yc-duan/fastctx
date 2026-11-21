//! The second version's library drills (design §28, plan 3-6): each one a
//! module that runs a few libraries in one process — a producer, a hub, a
//! replica — through what it drills, asserting as it goes. They run one
//! after another; every one that fails is told, and the test fails if any
//! did. Run with the `drill` feature (the library's raw commit):
//! `cargo test -p fastctx-content --features drill --test v2_drills`.

mod aside;
mod changes;
mod commit;
mod copies;
mod defer;
mod exchange;
mod faults;
mod hub;
mod judged;
mod letgo;
mod moves;
mod networks;
mod objects;
mod reads_ops;
mod rejected;
mod review;
mod snapshot;
mod succession;
mod transfer;
mod unaccepted;
mod vouch;
mod window;

fn main() {
    let drills: [(&str, fn()); 22] = [
        ("aside", aside::run as fn()),
        ("changes", changes::run as fn()),
        ("commit", commit::run as fn()),
        ("copies", copies::run as fn()),
        ("defer", defer::run as fn()),
        ("exchange", exchange::run as fn()),
        ("faults", faults::run as fn()),
        ("hub", hub::run as fn()),
        ("judged", judged::run as fn()),
        ("letgo", letgo::run as fn()),
        ("moves", moves::run as fn()),
        ("networks", networks::run as fn()),
        ("objects", objects::run as fn()),
        ("reads_ops", reads_ops::run as fn()),
        ("rejected", rejected::run as fn()),
        ("review", review::run as fn()),
        ("snapshot", snapshot::run as fn()),
        ("succession", succession::run as fn()),
        ("transfer", transfer::run as fn()),
        ("unaccepted", unaccepted::run as fn()),
        ("vouch", vouch::run as fn()),
        ("window", window::run as fn()),
    ];
    let mut failed = Vec::new();
    for (name, drill) in drills {
        match std::panic::catch_unwind(drill) {
            Ok(()) => println!("drill {name}: ok"),
            Err(_) => {
                println!("drill {name}: FAILED");
                failed.push(name);
            }
        }
    }
    if !failed.is_empty() {
        eprintln!("failed drills: {}", failed.join(", "));
        std::process::exit(1);
    }
}
