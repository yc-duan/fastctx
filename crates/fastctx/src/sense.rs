//! A conversation leaves a sentinel with its account's selected home.
use clap::Args;
use fastctx_types::{
    AccountName, Call, HubName, Reply,
    sentinel_control::{Create, Reply as SentinelReply, Request},
    sentinel_words as words,
};

#[derive(Args)]
#[command(disable_help_flag = true)]
pub(crate) struct SenseArgs {
    condition: Option<String>,
    #[arg(long)]
    at: Vec<String>,
    #[arg(long)]
    web: bool,
    #[arg(long, default_value = "")]
    tell: String,
    #[arg(long = "as")]
    account: Option<AccountName>,
    #[arg(long)]
    hub: Option<HubName>,
    #[arg(long, short = 'h')]
    help: bool,
}
pub(crate) fn run(args: SenseArgs) -> anyhow::Result<()> {
    let now = jiff::Timestamp::now();
    let request = if args.help {
        Request::Help
    } else {
        let at = args
            .at
            .iter()
            .map(|text| parse_time(text, now).map(|at| at.as_millisecond()))
            .collect::<anyhow::Result<Vec<_>>>()?;
        let request = Create { condition: args.condition.unwrap_or_default(), at, web: args.web, tell: args.tell };
        request.validate(now.as_millisecond()).map_err(anyhow::Error::msg)?;
        Request::Create { request }
    };
    let began = (!args.help).then(crate::steps::began).flatten();
    let (reply, _) = crate::ask::call(Call::Sentinel { hub: args.hub, account: args.account, request })?;
    match reply {
        Reply::Sentinel(reply) => match *reply {
            SentinelReply::Help { text } => crate::output::print(&text)?,
            SentinelReply::Created { sentinel } => {
                crate::output::print(&words::created(&sentinel))?;
                if let Some(began) = began {
                    began.tell(fastctx_types::run_facts::StepDid::Sense {
                        sentinel: sentinel.definition.id.to_string(),
                        condition: fastctx_types::run_facts::clipped(&sentinel.definition.condition),
                    });
                }
            }
            _ => anyhow::bail!("the node did not answer sentinel registration"),
        },
        _ => anyhow::bail!("the node did not answer sentinel registration"),
    }
    Ok(())
}
fn parse_time(text: &str, now: jiff::Timestamp) -> anyhow::Result<jiff::Timestamp> {
    if let Ok(at) = text.parse::<jiff::Timestamp>() {
        return Ok(at);
    }
    let zone = jiff::tz::TimeZone::system();
    if let Ok(at) = text.parse::<jiff::civil::DateTime>() {
        return Ok(at.to_zoned(zone)?.timestamp());
    }
    if let Ok(time) = text.parse::<jiff::civil::Time>() {
        let mut date = now.to_zoned(zone.clone()).date();
        let mut at = date.to_datetime(time).to_zoned(zone.clone())?.timestamp();
        if at <= now {
            date = date.checked_add(jiff::Span::new().days(1))?;
            at = date.to_datetime(time).to_zoned(zone)?.timestamp();
        }
        return Ok(at);
    }
    let delay: jiff::SignedDuration = text.parse().map_err(|e| {
        anyhow::anyhow!("invalid --at {text:?}: expected 08:00, 2026-10-23T08:00, an RFC 3339 timestamp, or 30m ({e})")
    })?;
    anyhow::ensure!(delay > jiff::SignedDuration::ZERO, "--at duration must be positive");
    Ok(now.checked_add(delay)?)
}

pub(crate) fn stop(
    runtime: &tokio::runtime::Runtime,
    client: &mut fastctx_client::Client,
    named: &str,
    machine: Option<&str>,
    hub: Option<&HubName>,
    account: Option<&AccountName>,
) -> anyhow::Result<bool> {
    let (id, qualified) = named.split_once('@').map_or((named, None), |(id, machine)| (id, Some(machine)));
    let home = qualified.or(machine).map(fastctx_types::access::Written::parse).transpose()?;
    let qualified_hub = home.as_ref().and_then(|h| h.hub.as_ref());
    anyhow::ensure!(
        hub.is_none() || qualified_hub.is_none() || hub == qualified_hub,
        "--hub differs from the sentinel's qualified home"
    );
    let id = id.parse()?;
    let reply = runtime.block_on(client.call(Call::Sentinel {
        hub: qualified_hub.or(hub).cloned(),
        account: account.cloned(),
        request: Request::Stop { id, home: home.map(|h| h.name) },
    }));
    match reply {
        Ok(Reply::Sentinel(reply)) => match *reply {
            SentinelReply::Stopped { id, reason, .. } => {
                crate::output::print(&words::stopped(&id, reason.as_deref()))?;
                Ok(true)
            }
            _ => anyhow::bail!("the node did not confirm sentinel withdrawal"),
        },
        Err(fastctx_client::CallError::Failed(error)) => {
            eprintln!("fastctx stop: {named}: {}", error.message);
            Ok(false)
        }
        Err(error) => Err(error.into()),
        _ => anyhow::bail!("the node did not confirm sentinel withdrawal"),
    }
}
