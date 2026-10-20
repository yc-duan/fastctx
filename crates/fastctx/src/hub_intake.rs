//! `fastctx update --hub-intake`: how fast the hub takes a new release in,
//! its super administrator's to raise for a server that takes more, or to
//! lower (`world-network.md`, 中枢替网络存的东西).

use anyhow::{Context as _, bail};
use fastctx_types::accounts::{AccountChange, Level};
use fastctx_types::update::{INTAKE, INTAKE_RANGE};
use fastctx_types::{Call, HubName, Reply};

/// A ceiling in bytes a second; none, the hub's default.
#[derive(Clone, Copy, Debug)]
pub struct Ceiling(pub Option<u64>);

/// A ceiling as the command line takes it: megabytes a second, or
/// `default`.
pub fn parser(text: &str) -> Result<Ceiling, String> {
    let text = text.trim();
    if text.eq_ignore_ascii_case("default") {
        return Ok(Ceiling(None));
    }
    let megabytes: f64 = text.parse().map_err(|_| format!("{text} is no number of megabytes a second"))?;
    let rate = (megabytes * 1_000_000.0).round();
    if !rate.is_finite() || rate < *INTAKE_RANGE.start() as f64 || rate > *INTAKE_RANGE.end() as f64 {
        return Err(format!(
            "the hub takes a release in at {} to {} MB a second",
            megabytes_of(*INTAKE_RANGE.start()),
            megabytes_of(*INTAKE_RANGE.end())
        ));
    }
    Ok(Ceiling(Some(rate as u64)))
}

/// Sets it as the super administrator of `hub`, or of the one hub where
/// this computer is logged in as one.
pub fn set(hub: Option<HubName>, rate: Option<u64>) -> anyhow::Result<()> {
    let Reply::Accounts(reply) = crate::ask::call(Call::Accounts)?.0 else { bail!("the node answered something else") };
    let mut theirs = reply.hubs.iter().filter_map(|seen| {
        let admin = seen
            .accounts
            .iter()
            .find(|account| account.level == Level::Super && seen.logins.contains(&account.name))?;
        hub.as_ref().is_none_or(|hub| seen.name == *hub).then(|| (seen.name.clone(), admin.name.clone()))
    });
    let (name, admin) =
        theirs.next().context("only the network's super administrator sets how fast its hub takes releases in")?;
    if hub.is_none() && theirs.next().is_some() {
        bail!("this computer administers more than one hub; name one with --hub");
    }
    match crate::ask::call(Call::Account {
        hub: Some(name.clone()),
        account: admin,
        change: AccountChange::Intake { rate },
    })?
    .0
    {
        Reply::Changed { .. } => {}
        other => bail!("the node answered something else: {other:?}"),
    }
    match rate {
        Some(rate) => println!("the hub {name} takes a new release in at {} MB a second at most", megabytes_of(rate)),
        None => println!(
            "the hub {name} takes a new release in at {} MB a second at most, its default",
            megabytes_of(INTAKE)
        ),
    }
    Ok(())
}

pub fn megabytes_of(rate: u64) -> String {
    let megabytes = rate as f64 / 1_000_000.0;
    if megabytes.fract() == 0.0 { format!("{megabytes:.0}") } else { format!("{megabytes:.1}") }
}
