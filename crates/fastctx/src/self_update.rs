//! `fastctx update`: new versions of Fastctx for a machine without the
//! graphical settings, such as a server reached over SSH
//! (`world-upgrade.md`, 没有界面的服务器). The node looks on GitHub and asks
//! the network, brings the command-line program here, checks its signature
//! and hands it to the same renewal an application's program takes.

use std::io::{IsTerminal as _, Write as _};

use fastctx_types::update::{Channel, Settings, State, UpdateCall};
use fastctx_types::{Call, Reply};

#[derive(clap::Args)]
pub struct UpdateArgs {
    /// Only look for a newer version and say what was found.
    #[arg(long)]
    check: bool,
    /// Take new versions by itself as they appear.
    #[arg(long, value_enum)]
    auto: Option<Switch>,
    /// Take test releases: on, off, or as the network says.
    #[arg(long, value_enum)]
    beta: Option<Beta>,
    /// Fetch packages for the network's machines that cannot reach GitHub.
    #[arg(long, value_enum)]
    relay: Option<Switch>,
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum Switch {
    On,
    Off,
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum Beta {
    On,
    Off,
    Network,
}

pub fn run(args: UpdateArgs) -> anyhow::Result<()> {
    let mut state = call(UpdateCall::State)?;
    if args.auto.is_some() || args.beta.is_some() || args.relay.is_some() {
        let mut settings: Settings = state.settings.clone();
        if let Some(auto) = args.auto {
            settings.auto = matches!(auto, Switch::On);
        }
        if let Some(beta) = args.beta {
            settings.beta = match beta {
                Beta::On => Some(true),
                Beta::Off => Some(false),
                Beta::Network => None,
            };
        }
        if let Some(relay) = args.relay {
            settings.relays = matches!(relay, Switch::On);
        }
        state = call(UpdateCall::Set { settings })?;
        print_settings(&state);
        return Ok(());
    }
    state = call(UpdateCall::Look)?;
    print_state(&state);
    if args.check {
        return Ok(());
    }
    let running = env!("CARGO_PKG_VERSION");
    let Some(offer) = state.offer.clone().filter(|offer| offer.newer_than(running)) else {
        println!("nothing to update");
        return Ok(());
    };
    if let Some(app) = &state.app {
        anyhow::bail!(
            "the application here, Fastctx {}, carries this machine's World program: update the application",
            app.version
        );
    }
    println!("updating to {}", offer.version);
    let updating = std::thread::spawn(|| call(UpdateCall::Kernel));
    let live = std::io::stdout().is_terminal();
    while !updating.is_finished() {
        std::thread::sleep(std::time::Duration::from_millis(500));
        if live
            && let Ok(state) = call(UpdateCall::State)
            && let Some(fetching) = &state.fetching
        {
            let via = match fetching.via {
                fastctx_types::update::Via::Github => "GitHub",
                fastctx_types::update::Via::Network => "the network",
            };
            let total = fetching.total.map(|total| format!(" of {}", size(total))).unwrap_or_default();
            print!("\r  from {via}: {}{total}   ", size(fetching.received));
            let _ = std::io::stdout().flush();
        }
    }
    if live {
        println!();
    }
    updating.join().map_err(|_| anyhow::anyhow!("the update stopped"))??;
    println!("fastctx {} is in place; it takes over once the turns under way here end", offer.version);
    Ok(())
}

fn call(call: UpdateCall) -> anyhow::Result<State> {
    match crate::ask::call(Call::Update(call))?.0 {
        Reply::Update(state) => Ok(*state),
        other => anyhow::bail!("the node answered {other:?}"),
    }
}

fn channel(channel: Channel) -> &'static str {
    match channel {
        Channel::Stable => "stable",
        Channel::Beta => "beta",
    }
}

fn print_settings(state: &State) {
    let beta = match state.settings.beta {
        Some(true) => "on",
        Some(false) => "off",
        None => "as the network says",
    };
    println!("channel: {} (test releases {beta})", channel(state.channel));
    println!("automatic updates: {}", if state.settings.auto { "on" } else { "off" });
    println!("fetching for the network: {}", if state.settings.relays { "on" } else { "off" });
}

fn print_state(state: &State) {
    println!("this machine runs fastctx {} on the {} channel", env!("CARGO_PKG_VERSION"), channel(state.channel));
    match (&state.looked.github, &state.looked.why) {
        (Some(true), None) => {
            let newest = state.looked.on(state.channel).map_or("no release", |manifest| manifest.version.as_str());
            println!("GitHub: newest {newest}");
        }
        (_, Some(why)) => println!("GitHub: {why}"),
        _ => println!("GitHub: not looked at yet"),
    }
    if let Some(network) = &state.network {
        let newest = network.on(state.channel).map_or("no release", |manifest| manifest.version.as_str());
        match &network.policy.minimum {
            Some(minimum) => println!("the network: newest {newest}; it asks for at least {minimum}"),
            None => println!("the network: newest {newest}"),
        }
    }
}

fn size(bytes: u64) -> String {
    match bytes {
        0..1_000_000 => format!("{:.0} KB", bytes as f64 / 1_000.0),
        _ => format!("{:.1} MB", bytes as f64 / 1_000_000.0),
    }
}
