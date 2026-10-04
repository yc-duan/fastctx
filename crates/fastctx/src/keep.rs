//! Keeping this machine connected once it is in a World: the node's service
//! is installed (`fastctx_install::service::keep_connected`), and the node
//! that served until then hands the World to the service's node.

use fastctx_install::apply::Places;
use fastctx_install::service::{Kept, keep_connected as keep};
use fastctx_types::Call;

pub fn keep_connected() -> anyhow::Result<()> {
    let places = Places::here()?;
    let kept_already = fastctx_types::layout::WorldDir::under(&places.root).node_service().exists();
    if cfg!(windows) && !kept_already && !fastctx_platform::elevation::is_elevated()? {
        println!(
            "Keeping this machine connected when no one is signed in takes a system service; \
             Windows asks for administrator rights once to install it."
        );
    }
    let (kept, report) = keep(&places)?;
    for leftover in &report.leftovers {
        println!("could not clear: {leftover}");
    }
    match kept {
        Kept::Already => println!("A system service keeps this machine's node running, signed in or not."),
        Kept::Installed => {
            // The service's node waits for the World, which this one hands
            // over once it is idle.
            crate::ask::call(Call::HandOver)?;
            println!("A system service keeps this machine's node running from now on, signed in or not.");
        }
        Kept::Declined => println!(
            "Not installed: the node runs while you are signed in. Run `fastctx service node install` as \
             administrator to install it later."
        ),
        Kept::Unsupported => println!("This system has no service for the node: it runs while you are signed in."),
        Kept::Command(command) => println!(
            "Keeping this machine connected when no one is signed in takes a system service. Install it with:\n  {command}"
        ),
    }
    Ok(())
}
