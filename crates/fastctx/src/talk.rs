//! `fastctx talk`: the user speaking to a resident anywhere in the World and
//! watching it (`world-colleagues.md`, 用户在任何地方和居民说话).
//!
//! With words, it says them as the user, shows what came before in the
//! session, and follows the turn they went into until it ends. Without
//! words, it watches the session and says each line typed, until input
//! ends. With `--open`, it first opens a session for the user. It is the
//! user's own voice, so it does not work in a command a resident runs.

use std::collections::{HashMap, HashSet};
use std::io::{BufRead as _, Write as _};

use anyhow::{Context as _, bail};
use clap::Args;
use fastctx_client::{Conversation, Next, Said, World};
use fastctx_hands::told::turn_end;
use fastctx_types::residents::Landed;
use fastctx_types::session::{Happened, Part, SessionChange, SessionEvent, Speaker, StepStatus, ToolStatus, What};
use fastctx_types::{AgentName, ResidentId, TownName};

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
    /// With --open: the model, as the agent names it; its default without it.
    #[arg(long, requires = "open")]
    model: Option<String>,
    /// With --open: how hard it reasons, as the agent names it (low, medium,
    /// high); the model's default without it.
    #[arg(long, requires = "open")]
    effort: Option<String>,
    /// The resident, such as codex-7f3a or codex-7f3a@laptop, then the words
    /// to say; with --open, only the words.
    args: Vec<String>,
}

pub fn run(args: TalkArgs) -> anyhow::Result<()> {
    if let Ok(resident) = std::env::var("FASTCTX_RESIDENT") {
        bail!(
            "fastctx talk is for the user to speak with, not for {resident}; to message a colleague, use message_colleague"
        );
    }
    let world = World::here()?;
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    runtime.block_on(async {
        let mut printer = Printer::default();
        if let Some(agent) = &args.open {
            let text = args.args.join(" ");
            if text.trim().is_empty() {
                bail!("say something to start the session with");
            }
            let options = fastctx_types::agents::SessionOptions {
                model: args.model.clone(),
                reasoning_effort: args.effort.clone(),
                ..Default::default()
            };
            let said = world
                .open_with_options(
                    AgentName::try_from(agent.as_str())?,
                    args.town.as_deref().map(TownName::try_from).transpose()?,
                    args.node.as_deref().map(fastctx_types::access::Written::parse).transpose()?,
                    text.clone(),
                    options,
                )
                .await?;
            println!("=== opened {} on {} ===", said.to.resident, said.to.machine);
            let (mut conversation, history) = Conversation::watch(
                &world,
                said.to.resident.clone(),
                Some(
                    args.node
                        .as_deref()
                        .map(fastctx_types::access::Written::parse)
                        .transpose()?
                        .unwrap_or_else(|| said.to.machine.clone().into()),
                ),
            )
            .await?;
            printer.events(&history);
            if !conversation.said(&said, &text) {
                printer.note(&gone(&said));
                return Ok(());
            }
            if conversation.ended_among(&history) {
                return Ok(());
            }
            return follow(&mut conversation, &mut printer).await;
        }
        let (target, words) = args.args.split_first().context("name the resident to talk to")?;
        let (resident, machine) = match target.split_once('@') {
            Some((resident, machine)) => (resident.parse()?, Some(fastctx_types::access::Written::parse(machine)?)),
            None => (ResidentId::try_from(target.as_str())?, None),
        };
        let (mut conversation, history) = Conversation::watch(&world, resident.clone(), machine.clone()).await?;
        printer.events(&history);
        if !words.is_empty() {
            let text = words.join(" ");
            let said = world.talk(resident, machine, text.clone()).await?;
            if !conversation.said(&said, &text) {
                printer.note(&gone(&said));
                return Ok(());
            }
            return follow(&mut conversation, &mut printer).await;
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
                next = conversation.next() => match next? {
                    Next::Moment(moment) => printer.event(&moment.event),
                    other => printer.watch(&other),
                },
                line = typed.recv() => match line {
                    Some(line) if line.trim().is_empty() => {}
                    Some(line) => {
                        let said = world.talk(resident.clone(), machine.clone(), line.clone()).await?;
                        if !conversation.said(&said, &line) {
                            printer.note(&gone(&said));
                        }
                    }
                    None => return Ok(()),
                },
            }
        }
    })
}

/// Shows what happens until the turn the words went into ends.
async fn follow(conversation: &mut Conversation, printer: &mut Printer) -> anyhow::Result<()> {
    loop {
        match conversation.next().await? {
            Next::Moment(moment) => {
                printer.event(&moment.event);
                if moment.answered {
                    return Ok(());
                }
            }
            other => printer.watch(&other),
        }
    }
}

/// Where words are that went into no turn yet.
fn gone(said: &Said) -> String {
    let to = &said.to.resident;
    match &said.landed {
        Landed::Uncertain { why } => format!(
            "whether {to} received your words is unknown: {why}; they remain recorded and will not be submitted again"
        ),
        Landed::Waiting { why, answered } => {
            let wait = match why {
                Some(why) => format!("your words wait for {to}: {why}"),
                None => format!("your words wait to be handed to {to}"),
            };
            if *answered {
                format!("{wait}; the turn they go into can be followed with fastctx talk {to}")
            } else {
                format!("{wait}; its host does not tell the World when its turns end")
            }
        }
        _ => format!("your words went to {to} on {}", said.to.machine),
    }
}

/// Shows a session's events in the terminal as they come.
#[derive(Default)]
struct Printer {
    /// Messages whose text has been shown piece by piece as it was written.
    streamed: HashMap<(String, String), String>,
    /// Items whose text is not shown as it grows: thinking and plans.
    quiet: HashSet<(String, String)>,
    /// Tools shown as they began; a history holds only how they finished.
    tools: HashSet<(String, String)>,
    /// Whether the cursor is in the middle of a line.
    open_line: bool,
    /// Why the watch is broken, while it is.
    broken: Option<String>,
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
            Happened::DeliveryUncertain { why, .. } => {
                self.line(&format!("── delivery acceptance is unknown: {why}; the original words remain recorded"))
            }
            Happened::TurnStarted { .. } => self.line("── turn started"),
            Happened::Heard { from, text, .. } => {
                let from = match from {
                    Some(Speaker::User { machine }) => format!("you, on {machine}"),
                    Some(Speaker::Resident { address }) => format!("{} on {}", address.resident, address.machine),
                    Some(Speaker::Page { page }) => page.to_string(),
                    Some(Speaker::World) => "the World".to_string(),
                    None => "words".to_string(),
                };
                self.line(&format!("> {from}: {text}"));
            }
            Happened::Began { turn, item } => match &item.what {
                What::Thinking { .. } | What::Plan { .. } => {
                    self.quiet.insert((turn.clone(), item.id.clone()));
                }
                What::Message { text } if !text.is_empty() => self.chunk((turn.clone(), item.id.clone()), text),
                What::Tool(tool) => {
                    self.tools.insert((turn.clone(), item.id.clone()));
                    self.line(&format!("  · {} {}", tool.name, fastctx_client::shown::tool_detail(tool)))
                }
                _ => {}
            },
            Happened::Grew { turn, item, part: Part::Text, text, .. }
                if !self.quiet.contains(&(turn.clone(), item.clone())) =>
            {
                self.chunk((turn.clone(), item.clone()), text);
            }
            Happened::Grew { .. } | Happened::Updated { .. } => {}
            // A change is shown when it says something of the session's
            // health; the rest is state for richer clients.
            Happened::Changed { change, .. } => {
                if matches!(change, SessionChange::Status { waiting_for_user: Some(true), .. }) {
                    self.line("  · waiting for you");
                } else if let Some(health) = change.health() {
                    self.line(&format!("  ! {}", health.text));
                }
            }
            Happened::Realtime { item, complete: true } => {
                if let Some(text) = item.get("text").and_then(serde_json::Value::as_str) {
                    self.line(text);
                }
            }
            Happened::Realtime { .. } | Happened::RealtimeGrew { .. } => {}
            Happened::Finished { turn, item } => match &item.what {
                What::Message { text } => {
                    if let Some(written) = self.streamed.remove(&(turn.clone(), item.id.clone()))
                        && let Some(rest) = text.strip_prefix(&written)
                    {
                        print!("{rest}");
                        self.open_line = !text.ends_with('\n');
                        self.end_line();
                    } else {
                        self.line(text);
                    }
                }
                What::Plan { text } => self.line(&format!("── proposed plan\n{text}")),
                What::Tool(tool) => {
                    let detail = fastctx_client::shown::tool_detail(tool);
                    let failed = matches!(tool.status, ToolStatus::Failed | ToolStatus::Declined);
                    match (self.tools.remove(&(turn.clone(), item.id.clone())), failed) {
                        (_, true) => self.line(&format!("  ✗ {} {detail} failed", tool.name)),
                        (false, false) => self.line(&format!("  · {} {detail}", tool.name)),
                        (true, false) => {}
                    }
                }
                What::AgentActivity { path, activity, .. } => self.line(&format!("  · {path}: {activity}")),
                What::Compaction => self.line("── context compacted"),
                What::Other { host_kind } => self.line(&format!("  [{host_kind}]")),
                _ => {}
            },
            Happened::Steps { steps, .. } => {
                for step in steps {
                    let mark = match step.status {
                        StepStatus::Done => "✓",
                        StepStatus::Doing => "▸",
                        StepStatus::Pending => "·",
                    };
                    self.line(&format!("  {mark} {}", step.text));
                }
            }
            Happened::Context { .. } | Happened::Titled { .. } => {}
            Happened::TurnEnded { turn, end } => {
                self.streamed.retain(|(t, _), _| t != turn);
                self.quiet.retain(|(t, _)| t != turn);
                self.tools.retain(|(t, _)| t != turn);
                self.line(&format!("── {}", turn_end(end)));
            }
            Happened::Health { health, .. } => self.line(&format!("  ! {}", health.text)),
        }
    }

    fn chunk(&mut self, key: (String, String), text: &str) {
        self.streamed.entry(key).or_default().push_str(text);
        print!("{text}");
        self.open_line = !text.ends_with('\n');
        let _ = std::io::stdout().flush();
    }

    fn note(&mut self, text: &str) {
        self.line(&format!("=== {text} ==="));
    }

    /// That the watch broke and is being opened again, once for each
    /// reason; that it is back.
    fn watch(&mut self, next: &Next) {
        match next {
            Next::Interrupted(interruption) => {
                if self.broken.as_ref() != Some(&interruption.why) {
                    self.note(&format!("connection lost: {}; reconnecting", interruption.why));
                    self.broken = Some(interruption.why.clone());
                }
            }
            Next::Resumed => {
                if self.broken.take().is_some() {
                    self.note("reconnected");
                }
            }
            Next::Moment(_) => {}
        }
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
