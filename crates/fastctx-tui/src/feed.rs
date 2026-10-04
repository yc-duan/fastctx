//! What keeps the page on top current, and the work pages ask of the World.
//!
//! A page that shows who is where asks again every two seconds while it is
//! on top (`world-colleagues.md`: the town page asks the machines every two
//! seconds while it is open); it asks the next time only after the last
//! answer came, so a slow machine never piles asks up. A conversation is a
//! watch, not an ask. Leaving the page stops what fed it.

use std::time::Duration;

use fastctx_client::{Conversation, TownView, World};
use jiff::Timestamp;
use tokio::sync::mpsc::UnboundedSender;
use tokio::task::JoinHandle;

use crate::app::{Data, Done, Feed, Job};

/// How long a page that shows who is where waits between asks.
const EVERY: Duration = Duration::from_secs(2);

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
    if let Feed::Watch { resident, machine, .. } = feed {
        return watch(world, sender, page, resident, machine).await;
    }
    loop {
        let data = ask(&world, &feed).await;
        if sender.send((page, data)).is_err() {
            return;
        }
        tokio::time::sleep(EVERY).await;
    }
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
            let (nodes, towns) = tokio::join!(world.nodes(), world.towns());
            Data::Choices { nodes: nodes.map_err(text), towns: towns.map_err(text) }
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
    let (mut conversation, history) = match Conversation::watch(&world, resident, Some(machine)).await {
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
        let data = match conversation.next().await {
            Ok(Some(moment)) => Data::Event(moment.event),
            Ok(None) => Data::Unwatched(None),
            Err(error) => Data::Unwatched(Some(error.to_string())),
        };
        let ended = matches!(data, Data::Unwatched(_));
        if sender.send((page, data)).is_err() || ended {
            return;
        }
    }
}

/// Does a job and sends its answer to the page that asked.
pub fn run(world: World, sender: Sender, page: u64, job: Job) {
    tokio::spawn(async move {
        let result = work(&world, job.clone()).await.map_err(|error| error.to_string());
        let _ = sender.send((page, Data::Done { job, result }));
    });
}

async fn work(world: &World, job: Job) -> Result<Done, fastctx_client::Error> {
    Ok(match job {
        Job::Talk { to, machine, text } => Done::Said(world.talk(to, Some(machine), text).await?),
        Job::Stop { resident, machine } => Done::Stopped(world.stop(resident, Some(machine)).await?.1),
        Job::Open { agent, town, node, text } => Done::Opened(world.open(agent, town, node, text).await?),
        Job::OpenTown { town, fields } => Done::Record(Box::new(world.open_town(town, fields).await?)),
        Job::EditTown { town, fields } => Done::Record(Box::new(world.edit_town(Some(town), fields).await?)),
        Job::Journal { town, journal, before } => {
            Done::Journal(Box::new(world.journal(Some(town), journal, before).await?))
        }
        Job::Invite { name, hours } => {
            let expires = Timestamp::now() + jiff::SignedDuration::from_hours(i64::from(hours));
            Done::Code { code: world.invite(name, expires).await?, expires }
        }
        Job::Join { code, name } => {
            let (world_id, name) = world.join(code, name).await?;
            Done::Joined { world: world_id, name }
        }
        Job::Revoke { name, reason } => {
            world.revoke(name, reason).await?;
            Done::Revoked
        }
        Job::Leave => {
            world.leave().await?;
            Done::Left
        }
    })
}
