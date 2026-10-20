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
    /// The release to update to; without one, the newest on the line of
    /// what runs here: test releases and stable ones for a test release,
    /// stable ones alone for a stable one.
    version: Option<String>,
    /// Only look, and list the releases this machine can update to.
    #[arg(long, conflicts_with = "version")]
    check: bool,
    /// Take new versions by itself as they appear.
    #[arg(long, value_enum, conflicts_with = "version")]
    auto: Option<Switch>,
    /// Fetch packages for the network's machines that cannot reach GitHub.
    #[arg(long, value_enum, conflicts_with = "version")]
    relay: Option<Switch>,
    /// How fast the hub takes a new release in at most, in MB a second, or
    /// `default` (5): its super administrator's to set.
    #[arg(long, value_name = "MB", value_parser = crate::hub_intake::parser, conflicts_with = "version")]
    hub_intake: Option<crate::hub_intake::Ceiling>,
    /// The hub `--hub-intake` sets, when this computer administers more than one.
    #[arg(long, requires = "hub_intake")]
    hub: Option<fastctx_types::HubName>,
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum Switch {
    On,
    Off,
}

pub fn run(args: UpdateArgs) -> anyhow::Result<()> {
    if let Some(crate::hub_intake::Ceiling(rate)) = args.hub_intake {
        return crate::hub_intake::set(args.hub, rate);
    }
    let mut state = call(UpdateCall::State)?;
    if args.auto.is_some() || args.relay.is_some() {
        let mut settings: Settings = state.settings.clone();
        if let Some(auto) = args.auto {
            settings.auto = matches!(auto, Switch::On);
            settings.chosen = true;
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
        print_releases(&state);
        return Ok(());
    }
    let running = env!("CARGO_PKG_VERSION");
    let version = match args.version {
        Some(version) => {
            let version = version.strip_prefix('v').unwrap_or(&version).to_string();
            if fastctx_types::update::release(&version) <= fastctx_types::update::release(running) {
                anyhow::bail!("this machine runs {running}; it does not go back to {version}");
            }
            version
        }
        None => match state.offer.clone().filter(|offer| offer.newer_than(running)) {
            Some(offer) => offer.version,
            None => {
                println!("nothing to update");
                return Ok(());
            }
        },
    };
    if let Some(app) = &state.app {
        anyhow::bail!(
            "the application here, Fastctx {}, carries this machine's World program: update the application",
            app.version
        );
    }
    println!("updating to {version}");
    let asked = Some(version.clone());
    let updating = std::thread::spawn(move || call(UpdateCall::Kernel { version: asked }));
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
    println!("fastctx {version} is in place; it takes over once the turns under way here end");
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
    let how = if state.settings.chosen { "" } else { " (by default)" };
    println!("automatic updates: {}{how}", if state.auto { "on" } else { "off" });
    println!("fetching for the network: {}", if state.settings.relays { "on" } else { "off" });
}

fn print_state(state: &State) {
    println!("this machine runs fastctx {} on the {} line", env!("CARGO_PKG_VERSION"), channel(state.channel));
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
        println!(
            "its hub takes a new release in at {} MB a second at most",
            crate::hub_intake::megabytes_of(network.policy.intake())
        );
    }
}

/// The releases newer than what runs here, the newest first.
fn print_releases(state: &State) {
    let running = fastctx_types::update::release(env!("CARGO_PKG_VERSION"));
    let mut newer: Vec<(String, bool)> = state
        .looked
        .releases
        .iter()
        .filter(|release| fastctx_types::update::release(&release.version) > running)
        .map(|release| (release.version.clone(), release.prerelease))
        .collect();
    if newer.is_empty()
        && let Some(network) = &state.network
    {
        for manifest in network.stable.iter().chain(network.beta.iter()) {
            if fastctx_types::update::release(&manifest.version) > running {
                newer.push((manifest.version.clone(), Channel::of(&manifest.version) == Channel::Beta));
            }
        }
        newer.sort_by(|a, b| fastctx_types::update::release(&b.0).cmp(&fastctx_types::update::release(&a.0)));
    }
    if newer.is_empty() {
        println!("no newer release");
        return;
    }
    println!("releases it can update to:");
    for (version, test) in newer {
        let major = fastctx_types::update::release(&version).map(|release| release.major);
        let past = state.hub_major.is_some_and(|hub| major.is_some_and(|major| major > hub));
        let notes = [test.then_some("test release"), past.then_some("newer than the hub's major version")];
        let notes: Vec<&str> = notes.into_iter().flatten().collect();
        match notes.is_empty() {
            true => println!("  {version}"),
            false => println!("  {version} ({})", notes.join(", ")),
        }
    }
    println!("`fastctx update <version>` updates to one of them");
}

fn size(bytes: u64) -> String {
    match bytes {
        0..1_000_000 => format!("{:.0} KB", bytes as f64 / 1_000.0),
        _ => format!("{:.1} MB", bytes as f64 / 1_000_000.0),
    }
}
