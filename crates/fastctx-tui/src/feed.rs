//! What keeps the page on top current, and the work pages ask of the World.
//!
//! A page that shows who is where asks again every two seconds while it is
//! on top (`world-colleagues.md`: the town page asks the machines every two
//! seconds while it is open); it asks the next time only after the last
//! answer came, so a slow machine never piles asks up. A conversation is a
//! watch, not an ask. Leaving the page stops what fed it.

use std::time::Duration;

use fastctx_client::{Conversation, TownView, World};
use fastctx_types::accounts::AccountChange;
use fastctx_types::{Address, MachineName, TownName};
use jiff::Timestamp;
use tokio::sync::mpsc::UnboundedSender;
use tokio::task::JoinHandle;

use crate::app::{Around, Data, Done, Feed, Job};
use crate::sessions::Refused;

/// How long a page that shows who is where waits between asks.
pub(crate) const EVERY: Duration = Duration::from_secs(2);
/// How many rounds pass between asks of this computer's agents: a minute.
const AGENTS_EVERY: u32 = 30;

pub type Sender = UnboundedSender<(u64, Data)>;

pub struct Feeds {
    world: World,
    sender: Sender,
    running: Option<((u64, Feed), JoinHandle<()>)>,
}

impl Feeds {
    pub fn new(world: World, sender: Sender) -> Self {
        Self { world, sender, running: None }
    }

    /// Feeds `wanted`, and stops feeding anything else.
    pub fn follow(&mut self, wanted: Option<(u64, Feed)>) {
        if self.running.as_ref().map(|(running, _)| running) == wanted.as_ref() {
            return;
        }
        if let Some((_, task)) = self.running.take() {
            task.abort();
        }
        if let Some((page, feed)) = wanted {
            let (world, sender) = (self.world.clone(), self.sender.clone());
            let task = tokio::spawn(feed_page(world, sender, page, feed.clone()));
            self.running = Some(((page, feed), task));
        }
    }
}

impl Drop for Feeds {
    fn drop(&mut self) {
        if let Some((_, task)) = self.running.take() {
            task.abort();
        }
    }
}

async fn feed_page(world: World, sender: Sender, page: u64, feed: Feed) {
    if let Feed::Watch { resident, machine, town, .. } = feed {
        let owner = Address { machine: machine.clone(), resident: resident.clone() };
        tokio::join!(
            watch(world.clone(), sender.clone(), page, resident, machine),
            around(world, sender, page, owner, town),
        );
        return;
    }
    let mut round: u32 = 0;
    loop {
        if matches!(feed, Feed::Nodes | Feed::World { .. }) {
            let status = fastctx_install::renewal::status(world.dir().path().parent().expect("local World root"))
                .await
                .map_err(|error| format!("{error:#}"));
            if sender.send((page, Data::Program(status))).is_err() {
                return;
            }
        }
        let data = ask(&world, &feed).await;
        let mut here = match &data {
            Data::Nodes(Ok(nodes)) => Some(nodes.machine.clone()),
            _ => None,
        };
        if sender.send((page, data)).is_err() {
            return;
        }
        // Beside the machines, how this one's own sessions stand.
        // The World page tells when an old fastctx's tools are still in
        // Codex; the machines page says how every host stands.
        let sessions = matches!(feed, Feed::Nodes | Feed::World { .. });
        if sessions && sender.send((page, Data::Sessions(crate::sessions::status().await))).is_err() {
            return;
        }
        // And this computer's agents, at first and then once a minute: asking
        // reads every login's usage from its agent's own servers.
        if round.is_multiple_of(AGENTS_EVERY)
            && let Some(machine) = here.take()
        {
            let agents = here_agents(&world, machine).await;
            if sender.send((page, Data::Agents(agents))).is_err() {
                return;
            }
        }
        round += 1;
        // And what its accounts see: who came in, and their levels.
        if matches!(feed, Feed::Nodes)
            && let Ok(accounts) = world.accounts().await
            && sender.send((page, Data::Accounts(accounts.hubs, accounts.dissolved))).is_err()
        {
            return;
        }
        tokio::time::sleep(EVERY).await;
    }
}

/// This computer's agents, as its node answers for it.
async fn here_agents(world: &World, machine: MachineName) -> Result<fastctx_types::agents::AgentsReply, String> {
    let agents = world.agents(Some(machine.clone().into())).await.map_err(|error| error.to_string())?;
    agents
        .machines
        .into_iter()
        .find_map(|answer| match answer {
            fastctx_types::agents::MachineAgents::Answered { reply, .. } if reply.machine == machine => Some(reply),
            _ => None,
        })
        .ok_or_else(|| format!("{machine} did not answer for its agents"))
}

async fn ask(world: &World, feed: &Feed) -> Data {
    let text = |error: fastctx_client::Error| error.to_string();
    match feed {
        Feed::World { all } => {
            let (who, nodes) = tokio::join!(world.who(None, *all), world.nodes());
            Data::World { view: who.map(|who| TownView::new(who, None)).map_err(text), nodes: nodes.map_err(text) }
        }
        Feed::Town { town, all } => {
            let (who, map) = tokio::join!(world.who(Some(town.clone()), *all), world.map(Some(town.clone())));
            Data::Town {
                view: who.map(|who| TownView::new(who, None)).map_err(text),
                map: map.map(Box::new).map_err(text),
            }
        }
        Feed::Nodes => Data::Nodes(world.nodes().await.map_err(text)),
        Feed::Choices => {
            let (nodes, towns, accounts) = tokio::join!(world.nodes(), world.towns(), world.accounts());
            // An assisting agent is offered when the accounts can be asked;
            // the machines and projects stand without it.
            let assisting = accounts
                .map(|accounts| {
                    accounts
                        .hubs
                        .iter()
                        .flat_map(|hub| {
                            hub.assists.iter().filter(|assist| hub.logins.contains(&assist.account)).map(|assist| {
                                crate::app::Assisting {
                                    machine: assist.machine.clone(),
                                    agent: assist.agent.clone(),
                                    home: assist.home.clone(),
                                    owner: hub
                                        .members
                                        .iter()
                                        .find(|member| member.name == assist.machine)
                                        .and_then(|member| member.home.clone()),
                                    to: fastctx_types::access::HubAccount {
                                        hub: hub.hub.clone(),
                                        account: assist.account.clone(),
                                    },
                                }
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            Data::Choices { nodes: nodes.map_err(text), towns: towns.map_err(text), assisting }
        }
        Feed::Watch { .. } => unreachable!("a watch is not asked"),
    }
}

async fn watch(
    world: World,
    sender: Sender,
    page: u64,
    resident: fastctx_types::ResidentId,
    machine: fastctx_types::MachineName,
) {
    let address = fastctx_types::Address { machine, resident };
    let (mut conversation, history) = match Conversation::open(&world, address.resident, address.machine.into()).await {
        Ok(watching) => watching,
        Err(error) => {
            let _ = sender.send((page, Data::Unwatched(Some(error.to_string()))));
            return;
        }
    };
    if sender.send((page, Data::History(history))).is_err() {
        return;
    }
    loop {
        let data = match conversation.update().await {
            Ok(fastctx_client::Update::Current) => Data::Current,
            Ok(fastctx_client::Update::Interrupted(interruption)) => {
                Data::Interrupted { why: interruption.why, since: interruption.since }
            }
            Ok(fastctx_client::Update::Moment(moment)) => Data::Event(moment.event),
            Ok(fastctx_client::Update::History(history)) => Data::History(history),
            // The listing says whether it works; the feed shows the session.
            Ok(fastctx_client::Update::Now { .. }) => continue,
            Ok(fastctx_client::Update::Extended(history)) => {
                for event in history {
                    if sender.send((page, Data::Event(event))).is_err() {
                        return;
                    }
                }
                continue;
            }
            Err(error) => Data::Unwatched(Some(error.to_string())),
        };
        let ended = matches!(data, Data::Unwatched(_));
        if sender.send((page, data)).is_err() || ended {
            return;
        }
    }
}

/// What `owner` started that still runs and the colleagues it asked for,
/// asked as often as a page that shows who is where, of the machines that
/// would list them.
async fn around(world: World, sender: Sender, page: u64, owner: Address, town: Option<TownName>) {
    loop {
        let around = world
            .who(town.clone(), false)
            .await
            .map(|who| {
                let view = TownView::new(who, None);
                Around {
                    jobs: view.jobs_by(&owner).into_iter().cloned().collect(),
                    colleagues: view.colleagues_of(&owner).into_iter().cloned().collect(),
                }
            })
            .map_err(|error| error.to_string());
        if sender.send((page, Data::Around(around))).is_err() {
            return;
        }
        tokio::time::sleep(EVERY).await;
    }
}

/// A computer logged in to a hub is kept connected by a system service;
/// the node that served until now hands the World over to it.
async fn keep_connected(world: &World) -> Result<fastctx_install::service::Kept, String> {
    let kept = tokio::task::spawn_blocking(|| {
        let places = fastctx_install::apply::Places::here().map_err(|error| error.to_string())?;
        fastctx_install::service::keep_connected(&places).map(|(kept, _)| kept).map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())
    .and_then(|kept| kept);
    match kept {
        Ok(fastctx_install::service::Kept::Installed) => world
            .hand_over()
            .await
            .map(|()| fastctx_install::service::Kept::Installed)
            .map_err(|error| error.to_string()),
        other => other,
    }
}

/// Follows the interface's optional parts this computer shows, which its
/// graphical interface may change too. A watch the node ended, as when it
/// restarts, is taken up again as pages ask again, every `EVERY`.
pub async fn features(world: World, sender: UnboundedSender<fastctx_types::features::Features>) {
    loop {
        if let Ok(mut watch) = world.watch_features().await {
            while let Ok(features) = watch.next().await {
                if sender.send(features).is_err() {
                    return;
                }
            }
        }
        tokio::time::sleep(EVERY).await;
    }
}

/// Does a job and sends its answer to the page that asked.
pub fn run(world: World, sender: Sender, page: u64, job: Job) {
    tokio::spawn(async move {
        let result = match &job {
            Job::Apply { raise } => match crate::sessions::apply(*raise).await {
                Ok(report) => Ok(Done::Applied(report)),
                Err(Refused::OutputLimit { current, requested }) => Ok(Done::NeedsRaise { current, requested }),
                Err(Refused::Failed(why)) => Err(why),
            },
            Job::Unapply => crate::sessions::unapply().await.map(Done::Unapplied),
            Job::ApplyHome { host, home, into, raise, take_back, machine } => {
                let result = match crate::sessions::change_at(*host, home.clone(), *into, *raise, *take_back).await {
                    Ok(report) if *into => Ok(Done::Applied(report)),
                    Ok(report) => Ok(Done::Unapplied(report)),
                    Err(Refused::OutputLimit { current, requested }) => Ok(Done::NeedsRaise { current, requested }),
                    Err(Refused::Failed(why)) => Err(why),
                };
                // The login's line says at once how it stands now.
                let _ = sender.send((page, Data::Agents(here_agents(&world, machine.clone()).await)));
                result
            }
            _ => work(&world, job.clone()).await.map_err(|error| error.to_string()),
        };
        let _ = sender.send((page, Data::Done { job, result }));
    });
}

async fn work(world: &World, job: Job) -> Result<Done, fastctx_client::Error> {
    Ok(match job {
        Job::Talk { to, machine, text } => Done::Said(world.talk(to, Some(machine.into()), text).await?),
        Job::Stop { resident, machine } => Done::Stopped(world.stop(resident, Some(machine.into())).await?.1),
        Job::Attach { resident } => Done::Attachable(world.attach(resident).await?),
        Job::Open { agent, town, node, text, choices } => {
            Done::Opened(world.open_with(agent, town, node.map(Into::into), text, Default::default(), choices).await?)
        }
        Job::OpenTown { town, fields } => Done::Record(Box::new(world.open_town(town, fields, None, None).await?)),
        Job::EditTown { town, fields } => Done::Record(Box::new(world.edit_town(Some(town), fields).await?)),
        Job::Journal { town, journal, before } => {
            Done::Journal(Box::new(world.journal(Some(town), journal, before).await?))
        }
        Job::Login { hub, account, password, register, name } => {
            let addresses = fastctx_client::hub_addresses(&hub).await.map_err(fastctx_client::Error::Other)?;
            let (hub, name, account) = world.login(addresses, account, password, register, name).await?;
            Done::LoggedIn { hub, name, account, kept: keep_connected(world).await }
        }
        Job::ServerLine { hub, account, hours } => {
            let expires = Timestamp::now() + jiff::SignedDuration::from_hours(i64::from(hours));
            Done::ServerLine {
                text: world.server_command(Some(hub), account, None, expires, false, None, None, false, false).await?,
                expires,
            }
        }
        Job::Device { hub, account } => {
            let accounts = world.accounts().await?;
            let addresses = accounts
                .hubs
                .iter()
                .find(|seen| seen.name == hub)
                .map(|seen| seen.addresses.iter().map(ToString::to_string).collect())
                .unwrap_or_default();
            Done::Device { addresses, account }
        }
        Job::Logout { hub, account } => {
            world.account(Some(hub), account, AccountChange::Logout).await?;
            Done::Changed
        }
        Job::Remove { hub, account, name, reason, seen } => {
            world
                .account(
                    Some(hub.clone()),
                    account.clone(),
                    AccountChange::Remove { name: name.clone(), reason, then: None },
                )
                .await?;
            if seen {
                world.account(Some(hub), account, AccountChange::Seen { name }).await?;
            }
            Done::Changed
        }
        Job::Seen { hub, account, name } => {
            world.account(Some(hub), account, AccountChange::Seen { name }).await?;
            Done::Changed
        }
        Job::SeenDissolved => {
            world.seen_dissolved().await?;
            Done::Changed
        }
        Job::Conversations { on } => {
            world.set_feature(fastctx_types::features::Feature::Conversations, on).await?;
            Done::Changed
        }
        Job::Apply { .. } | Job::Unapply | Job::ApplyHome { .. } => {
            unreachable!("done on this machine, not asked of the World")
        }
    })
}
