//! `fastctx talk`: the user speaking to a resident anywhere in the World and
//! watching it (`world-colleagues.md`, 用户在任何地方和居民说话).
//!
//! With words, it says them as the user, shows what came before in the
//! session, and follows the turn they went into until it ends. Without
//! words, it watches the session and says each line typed, until input
//! ends. With `--open`, it first opens a session for the user. It is the
//! user's own voice, so it does not work in a command a resident runs.

use std::collections::HashSet;
use std::io::{BufRead as _, Write as _};

use anyhow::{Context as _, bail};
use clap::Args;
use fastctx_client::{Client, Watching};
use fastctx_hands::told::turn_end;
use fastctx_types::layout::WorldDir;
use fastctx_types::residents::Landed;
use fastctx_types::session::{Happened, SessionEvent, Speaker, Watched};
use fastctx_types::{Address, AgentName, Call, MachineName, Reply, ResidentId, TownName};

/// Talk to a resident on any machine, or open a session to talk to.
#[derive(Args)]
pub struct TalkArgs {
    /// Open a session for you first: `codex`.
    #[arg(long, value_name = "AGENT")]
    open: Option<String>,
    /// With --open: the town it works in.
    #[arg(long, requires = "open")]
    town: Option<String>,
    /// With --open: the machine it runs on; this one without it.
    #[arg(long, requires = "open")]
    node: Option<String>,
    /// The resident, such as codex-7f3a or codex-7f3a@laptop, then the words
    /// to say; with --open, only the words.
    args: Vec<String>,
}

pub fn run(args: TalkArgs) -> anyhow::Result<()> {
    if let Ok(resident) = std::env::var("FASTCTX_RESIDENT") {
        bail!(
            "{resident} runs this; fastctx talk is the user's own voice. Residents speak to each other with message_colleague"
        );
    }
    let world = WorldDir::under(&fastctx_platform::fastctx_root()?);
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    runtime.block_on(async {
        let mut printer = Printer::default();
        if let Some(agent) = &args.open {
            let text = args.args.join(" ");
            if text.trim().is_empty() {
                bail!("say something to start the session with");
            }
            let call = Call::Open {
                agent: AgentName::try_from(agent.as_str())?,
                town: args.town.as_deref().map(TownName::try_from).transpose()?,
                node: args.node.as_deref().map(MachineName::try_from).transpose()?,
                text,
            };
            let (resident, landed) = match client(&world).await?.call(call).await? {
                Reply::Opened { resident, landed } => (resident, landed),
                other => bail!("the node answered something else: {other:?}"),
            };
            println!("=== opened {} on {} ===", resident.resident, resident.machine);
            let mut watching = watch(&world, &resident.resident, Some(resident.machine.clone())).await?;
            let history = first(&mut watching).await?;
            let turn = turn_of(&landed);
            let ended = printer.events(&history);
            if turn.as_ref().is_some_and(|turn| ended.contains(turn)) {
                return Ok(());
            }
            return follow(&mut watching, &mut printer, turn, &resident).await;
        }
        let (target, words) = args.args.split_first().context("name the resident to talk to")?;
        let (resident, machine) = match target.parse::<Address>() {
            Ok(address) => (address.resident, Some(address.machine)),
            Err(_) => (ResidentId::try_from(target.as_str())?, None),
        };
        let mut watching = watch(&world, &resident, machine.clone()).await?;
        let history = first(&mut watching).await?;
        printer.events(&history);
        if !words.is_empty() {
            let (to, landed) = say(&world, &resident, machine, words.join(" ")).await?;
            return follow(&mut watching, &mut printer, turn_of(&landed), &to).await;
        }
        // No words: say each line typed, and show everything until input
        // ends.
        let (lines, mut typed) = tokio::sync::mpsc::unbounded_channel::<String>();
        std::thread::spawn(move || {
            for line in std::io::stdin().lock().lines() {
                match line {
                    Ok(line) => {
                        if lines.send(line).is_err() {
                            return;
                        }
                    }
                    Err(_) => return,
                }
            }
        });
        loop {
            tokio::select! {
                piece = watching.next() => match piece? {
                    Some(Watched::Live { event }) => printer.event(&event),
                    Some(Watched::History { events }) => { printer.events(&events); }
                    None => return Ok(()),
                },
                line = typed.recv() => match line {
                    Some(line) if line.trim().is_empty() => {}
                    Some(line) => {
                        let (to, landed) = say(&world, &resident, machine.clone(), line).await?;
                        if let Landed::Waiting { why } = landed {
                            printer.note(&waiting(&to, why));
                        }
                    }
                    None => return Ok(()),
                },
            }
        }
    })
}

async fn client(world: &WorldDir) -> anyhow::Result<Client> {
    Ok(Client::connect_or_start(world, fastctx_client::command_peer()).await?)
}

async fn watch(world: &WorldDir, resident: &ResidentId, machine: Option<MachineName>) -> anyhow::Result<Watching> {
    let call = Call::Watch { resident: resident.clone(), machine };
    Ok(client(world).await?.watch(call).await?)
}

/// The history every watch starts with.
async fn first(watching: &mut Watching) -> anyhow::Result<Vec<SessionEvent>> {
    match watching.next().await? {
        Some(Watched::History { events }) => Ok(events),
        Some(Watched::Live { event }) => Ok(vec![event]),
        None => bail!("the watch ended before it began"),
    }
}

async fn say(
    world: &WorldDir,
    resident: &ResidentId,
    machine: Option<MachineName>,
    text: String,
) -> anyhow::Result<(Address, Landed)> {
    match client(world).await?.call(Call::Talk { to: resident.clone(), machine, text }).await? {
        Reply::Spoken { to, landed } => Ok((to, landed)),
        other => bail!("the node answered something else: {other:?}"),
    }
}

/// Shows what happens until the turn `turn` ends; with no turn to follow,
/// says why and stops.
async fn follow(
    watching: &mut Watching,
    printer: &mut Printer,
    turn: Option<String>,
    to: &Address,
) -> anyhow::Result<()> {
    let Some(turn) = turn else {
        printer
            .note(&format!("{} cannot be followed here: its session tells the World nothing as it works", to.resident));
        return Ok(());
    };
    loop {
        match watching.next().await? {
            Some(Watched::Live { event }) => {
                printer.event(&event);
                // An empty turn is the next one to end.
                if matches!(&event.happened, Happened::TurnEnded { turn: ended, .. } if turn.is_empty() || *ended == turn)
                {
                    return Ok(());
                }
            }
            Some(Watched::History { events }) => {
                if printer.events(&events).contains(&turn) {
                    return Ok(());
                }
            }
            None => return Ok(()),
        }
    }
}

/// Which turn to follow: the one words went into, the next one to end when
/// they were handed to a session's plugin, or none.
fn turn_of(landed: &Landed) -> Option<String> {
    match landed {
        Landed::NewTurn { turn } | Landed::JoinedTurn { turn } => Some(turn.clone()),
        Landed::Handed => Some(String::new()),
        Landed::Waiting { .. } | Landed::Sent => None,
    }
}

fn waiting(to: &Address, why: Option<String>) -> String {
    match why {
        Some(why) => format!("your words wait for {}: {why}", to.resident),
        None => format!("your words wait for {}'s next call of a World tool", to.resident),
    }
}

/// Shows a session's events in the terminal as they come.
#[derive(Default)]
struct Printer {
    /// Messages whose text has been shown piece by piece as it was written.
    streamed: HashSet<String>,
    /// Whether the cursor is in the middle of a line.
    open_line: bool,
}

impl Printer {
    /// Shows past events; returns the turns that ended among them.
    fn events(&mut self, events: &[SessionEvent]) -> HashSet<String> {
        let mut ended = HashSet::new();
        for event in events {
            if let Happened::TurnEnded { turn, .. } = &event.happened {
                ended.insert(turn.clone());
            }
            self.event(event);
        }
        ended
    }

    fn event(&mut self, event: &SessionEvent) {
        match &event.happened {
            Happened::TurnStarted { .. } => self.line("── turn started"),
            Happened::Heard { from, text, .. } => {
                let from = match from {
                    Some(Speaker::User { machine }) => format!("you, on {machine}"),
                    Some(Speaker::Resident { address }) => format!("{} on {}", address.resident, address.machine),
                    Some(Speaker::World) => "the World".to_string(),
                    None => "words".to_string(),
                };
                self.line(&format!("> {from}: {text}"));
            }
            Happened::Writing { item, delta, .. } => {
                self.streamed.insert(item.clone());
                print!("{delta}");
                self.open_line = !delta.ends_with('\n');
                let _ = std::io::stdout().flush();
            }
            Happened::Said { item, text, .. } => {
                if self.streamed.remove(item) {
                    self.end_line();
                } else {
                    self.line(text);
                }
            }
            Happened::ToolStarted { tool, detail, .. } => self.line(&format!("  · {tool} {detail}")),
            Happened::ToolEnded { tool, detail, failed: true, .. } => self.line(&format!("  ✗ {tool} {detail} failed")),
            Happened::ToolEnded { .. } => {}
            Happened::TurnEnded { end, .. } => self.line(&format!("── {}", turn_end(end))),
            Happened::Health { health, .. } => self.line(&format!("  ! {}", health.text)),
        }
    }

    fn note(&mut self, text: &str) {
        self.line(&format!("=== {text} ==="));
    }

    fn line(&mut self, text: &str) {
        self.end_line();
        println!("{text}");
    }

    fn end_line(&mut self) {
        if self.open_line {
            println!();
            self.open_line = false;
        }
    }
}
