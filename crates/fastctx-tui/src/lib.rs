//! The World in a terminal (`world-gui.md`): projects, the agents in them,
//! the machines, and conversations with any agent on any machine, drawn in
//! the World's design language and read through `fastctx_client`, the one
//! layer every interface reads the World through.

pub mod app;
pub mod design;
pub mod editor;
pub mod feed;
pub mod input;
pub mod markdown;
pub mod pages;
pub mod paint;
#[cfg(feature = "preview")]
pub mod preview;
pub mod sessions;
pub mod terminal;
pub mod text;
pub mod transcript;
pub mod who;
pub mod words;

use std::time::Duration;

use anyhow::Context as _;
use fastctx_client::World;
use tokio::sync::mpsc;
use tokio::time::MissedTickBehavior;

use crate::app::App;
use crate::design::Look;
use crate::feed::Feeds;
use crate::input::Input;
use crate::pages::Page;
use crate::pages::world::WorldPage;
use crate::terminal::Screen;
use crate::words::Words;

/// How often a mark of something happening is redrawn as it breathes: ten
/// frames a second is smooth for a slow fade and quiet for the terminal.
const BREATH: Duration = Duration::from_millis(100);

/// Runs the TUI until the user leaves it.
pub fn run() -> anyhow::Result<()> {
    let world = World::here().context("finding this user's World")?;
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    runtime.block_on(live(world))
}

async fn live(world: World) -> anyhow::Result<()> {
    // Read before anything changes the console's modes.
    let depth = design::depth().context("reading what the terminal can show")?;
    let look = Look { depth, theme: design::theme() };
    let words = Words::new(words::lang());
    let mut screen = Screen::open().context("taking the terminal")?;
    let (input_sender, mut inputs) = mpsc::unbounded_channel();
    input::start(input_sender).context("reading the terminal's input")?;
    let (answer_sender, mut answers) = mpsc::unbounded_channel();
    let mut app = App::new(look, words, Page::World(WorldPage::new()));
    let mut feeds = Feeds::new(world.clone(), answer_sender.clone());
    let mut breath = tokio::time::interval(BREATH);
    breath.set_missed_tick_behavior(MissedTickBehavior::Delay);
    // Times on the screen are read to the minute and counted in seconds.
    let mut clock = tokio::time::interval(Duration::from_secs(1));
    clock.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut failed = None;
    loop {
        feeds.follow(app.feed());
        app.tick();
        screen.terminal.draw(|frame| app.draw(frame))?;
        let jobs = tokio::select! {
            input = inputs.recv() => match input {
                Some(Input::Failed(why)) => {
                    failed = Some(why);
                    break;
                }
                Some(input) => app.input(input),
                None => break,
            },
            Some((page, data)) = answers.recv() => app.data(page, data),
            _ = breath.tick(), if app.animating() => Vec::new(),
            _ = clock.tick() => Vec::new(),
        };
        for (page, job) in jobs {
            feed::run(world.clone(), answer_sender.clone(), page, job);
        }
        // A turn that ended is told of only when the terminal is not in
        // front; a terminal that cannot be told is told nothing.
        for (title, body) in app.notices() {
            screen.notify(&title, &body)?;
        }
        if app.quit {
            break;
        }
    }
    drop(feeds);
    drop(screen);
    match failed {
        Some(why) => anyhow::bail!("the terminal's input could not be read: {why}"),
        None => Ok(()),
    }
}
