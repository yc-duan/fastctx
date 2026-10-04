//! Asking for a colleague and speaking to one (`world-colleagues.md`, 两个
//! 工具).
//!
//! A colleague is a Codex resident the World starts, on the asker's machine
//! or another, as a thread of that machine's app-server. The asker's words
//! become its first turn, and the asker is owed that turn's end. Asking is
//! recorded as one of the asker's actions: where the colleague lives, and
//! its id; speaking to it later starts from that record.

use std::path::PathBuf;
use std::sync::Arc;

use fastctx_hosts::codex::{self, ThreadSpec};
use fastctx_net::link::CallFailure;
use fastctx_net::message::LetterId;
use fastctx_types::ipc::Failure;
use fastctx_types::remote::{Letter, RemoteCall, RemoteReply};
use fastctx_types::residents::{
    About, Action, Activity, Delivery, Host, Landed, MessageKind, MessageLine, Notice, Now, Origin, Pending,
    PendingState,
};
use fastctx_types::session::{Speaker, TurnHow};
use fastctx_types::{Address, AgentName, MachineName, ResidentId, TownName};
use jiff::Timestamp;

use crate::Node;
use crate::residents::Heard;
use crate::speech::{self, Owed};

/// Asks for a colleague for `asker`, a resident of this machine, on this
/// machine or on `target`. Returns where the colleague lives.
pub async fn ask(
    node: &Arc<Node>,
    asker: ResidentId,
    town: Option<TownName>,
    label: String,
    prompt: String,
    target: Option<MachineName>,
) -> Result<Address, Failure> {
    let here = node.machine();
    let at = Timestamp::now();
    let colleague = match target.filter(|target| *target != here) {
        None => {
            let address = Address { machine: here.clone(), resident: asker.clone() };
            let id = ask_here(node, address, town.clone(), label.clone(), prompt).await?;
            Address { machine: here, resident: id }
        }
        Some(target) => {
            let link = node.link().ok_or_else(|| {
                Failure::new(format!("there is no machine {target}: {here} is in no World with other machines"))
            })?;
            let call = RemoteCall::Ask { asker: asker.clone(), town: town.clone(), label: label.clone(), prompt };
            match link.call(&target, call).await.map_err(|failure| Failure::new(failure.to_string()))? {
                Ok(RemoteReply::Asked { colleague }) => Address { machine: target, resident: colleague },
                Ok(_) => return Err(Failure::new(format!("{target} answered something other than a colleague"))),
                // What went wrong there is said of that machine, not the asker's.
                Err(failure) => return Err(Failure::new(format!("on {target}: {}", failure.message))),
            }
        }
    };
    let action = Action {
        at,
        machine: colleague.machine.clone(),
        hand: "ask_colleague".to_string(),
        what: label,
        result: colleague.resident.to_string(),
    };
    record(node, &asker, town, action).await;
    Ok(colleague)
}

/// Starts a colleague on this machine for `asker`, which may live on
/// another, with `prompt` as its first words. Returns its id once its
/// thread exists; the words go in after, as its tools may take a while to
/// start.
pub async fn ask_here(
    node: &Arc<Node>,
    asker: Address,
    town: Option<TownName>,
    label: String,
    prompt: String,
) -> Result<ResidentId, Failure> {
    node.residents.set_departed(asker.clone(), false);
    if let Some(town) = &town {
        crate::town::project::spans_here(node, town).await?;
    }
    let agent = AgentName::try_from("codex").expect("a valid agent name");
    let id = node.residents.new_id(&agent)?;
    let origin = Origin::Colleague { asked_by: asker.clone(), label };
    let (spec, heard) = thread_spec(node, &id, &origin, town.as_ref()).await?;
    let (thread, model) = node.codex.start_thread(spec).await.map_err(Failure::new)?;
    let born_at = Timestamp::now();
    let about = About {
        id: id.clone(),
        host: Host { agent, client: "codex app-server".to_string(), version: String::new() },
        machine: node.machine(),
        born_at,
        origin,
        session: Some(thread.clone()),
        world_started: true,
        model,
    };
    let now = Now {
        activity: Activity::Busy,
        since: born_at,
        turn: None,
        owed: Default::default(),
        answering: vec![asker.clone()],
        trouble: None,
        desk: false,
        waiting_for_user: false,
    };
    node.residents
        .create(&node.world, about.clone(), town.clone(), now)
        .map_err(|error| Failure::new(format!("recording the colleague: {error}")))?;
    node.residents.set_heard(&id, heard);
    crate::codex::claim(node, &thread, &id).await;
    let directory = node.world.resident(town.as_ref(), &id);
    let asked = MessageLine { at: born_at, what: MessageKind::Asked { from: asker.clone(), text: prompt.clone() } };
    if let Err(error) = crate::residents::append_message(&directory, &asked) {
        node.log.line(format_args!("recording what {id} was asked: {error}"));
    }
    // The request is kept before the asker hears of the colleague: a node
    // that stops before it goes in finds it, and puts it in then.
    let first = Pending {
        delivery: Delivery {
            id: speech::new_id("words"),
            at: born_at,
            notice: Notice::Words { from: Speaker::Resident { address: asker.clone() }, text: prompt },
        },
        readers: vec![asker],
        state: PendingState::Waiting,
        working: false,
        session: None,
    };
    crate::inbox::keep(&directory, &first).map_err(|error| Failure::new(format!("keeping the request: {error}")))?;
    let (node, colleague) = (node.clone(), id.clone());
    tokio::spawn(async move {
        let why = match speech::hand_over(&node, &colleague, &about, &directory, first.clone()).await {
            Ok(
                Landed::NewTurn { .. }
                | Landed::JoinedTurn { .. }
                | Landed::Sent
                | Landed::Handed { .. }
                | Landed::Uncertain { .. },
            ) => return,
            Ok(Landed::Waiting { why, .. }) => why.unwrap_or_else(|| "its session could not be reached".to_string()),
            Err(failure) => failure.message,
        };
        // Its first turn never began: whoever asked hears so, and it is idle.
        crate::turns::unsettled(&node, &colleague, &directory, first, TurnHow::Failed, why.clone()).await;
        crate::turns::ended(
            &node,
            &colleague,
            "",
            fastctx_types::session::TurnEnd {
                error: None,
                how: TurnHow::Failed,
                reason: Some(why),
                duration_ms: None,
                last_message: None,
            },
        )
        .await;
    });
    Ok(id)
}

/// What a thread the World holds is started and resumed with: where it is,
/// with its town's map, and whom it works for, and the World's tools under
/// its own name. Also what it is told of its town by that.
pub async fn thread_spec(
    node: &Arc<Node>,
    id: &ResidentId,
    origin: &Origin,
    town: Option<&TownName>,
) -> Result<(ThreadSpec, Heard), Failure> {
    let here = node.machine();
    let cwd = standing(node, town)?;
    let binary = node.fastctx.clone();
    let address = Address { machine: here.clone(), resident: id.clone() };
    let mut env = vec![
        ("FASTCTX_HOME".to_string(), node.root.to_string_lossy().into_owned()),
        ("FASTCTX_RESIDENT".to_string(), address.tag()),
    ];
    let mut tools = vec!["run", "apply_patch"];
    // A colleague's thread is listed under its task; a session the user
    // opens is named by whichever client shows it first, as Codex does.
    let name = match origin {
        Origin::Colleague { label, .. } => {
            env.push(("FASTCTX_COLLEAGUE".to_string(), "1".to_string()));
            Some(label.clone()).filter(|label| !label.trim().is_empty())
        }
        Origin::User => {
            tools.extend(["ask_colleague", "message_colleague"]);
            None
        }
    };
    let mut config = codex::world_tools(&binary, &env, &tools, fastctx_hands::run::window::BUDGET);
    crate::session_options::configure(&mut config, crate::session_options::read(node, id)?);
    let (developer_instructions, heard) = instructions(node, id, town, origin).await?;
    Ok((ThreadSpec { cwd, developer_instructions, config, name }, heard))
}

/// How far into its town's ledger a resident is told of it, as of now.
pub fn heard_now(node: &Node, town: Option<&TownName>) -> std::io::Result<Heard> {
    let ledger = match town {
        Some(town) => crate::ledger::seq(&node.world, town)?,
        None => 0,
    };
    Ok(Heard { town: town.cloned(), ledger })
}

/// The town's bookkeeping directory, where a resident the World starts
/// stands: it means nothing to the resident, but groups a town's sessions
/// together in the hosts' own lists (`world-town.md`).
pub(crate) fn standing(node: &Node, town: Option<&TownName>) -> Result<PathBuf, Failure> {
    let directory = match town {
        Some(town) => node.world.path().join("towns").join(town.as_str()),
        None => node.world.path().join("outside"),
    };
    fastctx_platform::ensure_private_dir(&directory)
        .map_err(|error| Failure::new(format!("making {}: {error}", directory.display())))?;
    Ok(directory)
}

/// What a resident the World starts is told of itself, as Codex's developer
/// instructions or Claude's system prompt, which outlive compaction: who it
/// is, its hands, and its town's map. Also what it is told of its town by
/// that: how far into the ledger, read before the map so nothing written
/// meanwhile goes unsaid.
pub(crate) async fn instructions(
    node: &Arc<Node>,
    id: &ResidentId,
    town: Option<&TownName>,
    origin: &Origin,
) -> Result<(String, Heard), Failure> {
    let heard = heard_now(node, town).map_err(|error| Failure::new(format!("reading the town's ledger: {error}")))?;
    let words = words(id, &node.machine(), town, origin);
    let text = match town {
        Some(town) => format!("{words}\n\n{}", crate::town::map::brief(node, town).await?),
        None => words,
    };
    Ok((text, heard))
}

fn words(id: &ResidentId, machine: &MachineName, town: Option<&TownName>, origin: &Origin) -> String {
    let place = match (town, origin) {
        (Some(town), _) => format!("You live on the machine {machine}, in the town {town}; its map follows."),
        (None, Origin::User) => format!(
            "You live on the machine {machine}, in no town yet. When the user names the project to work on, enter its town with `fastctx project enter <town>` (`fastctx project list` lists them)."
        ),
        (None, Origin::Colleague { .. }) => format!("You live on the machine {machine}, in no town."),
    };
    let working = match origin {
        Origin::Colleague { asked_by, .. } => format!(
            "{} on {} asked for you, and you work for it. What it asked comes next, under a line naming it. Words from anyone else arrive the same way. At the end of each turn, your last message goes word for word to whoever's words went into that turn, and to no one else: nothing you write before it reaches anyone. So put what you found, or what you need to ask, in that last message.",
            asked_by.resident, asked_by.machine
        ),
        Origin::User => "The user opened this session through the World and speaks to you here.".to_string(),
    };
    format!(
        "You are {id}, a resident of the World: the user's machines working as one place. {place}\n\n{working}\n\nYour hands are the World's tools. `run` runs a bash script on any machine of the World: `node` names the machine, and without it the script runs on this one. `apply_patch` edits files on any machine. There is no other shell or editor here. Paths are absolute, written the way the machine they are on writes them. A script still running after 15 seconds goes on in the background; you are told when it ends, so you can end your turn and you will be woken with the news."
    )
}

/// Words from `from`, a resident of this machine, to `to`, on any machine.
pub async fn message(
    node: &Arc<Node>,
    from: ResidentId,
    town: Option<TownName>,
    to: ResidentId,
    to_machine: Option<MachineName>,
    text: String,
) -> Result<(Address, Landed), Failure> {
    let here = node.machine();
    let at = Timestamp::now();
    let machine = match to_machine {
        Some(machine) => machine,
        None => locate(node, &from, town.as_ref(), &to).await?,
    };
    let to = Address { machine, resident: to };
    let sender = Address { machine: here, resident: from.clone() };
    let notice = Notice::Words { from: Speaker::Resident { address: sender.clone() }, text };
    let landed = speech::tell(node, &to, speech::new_id("words"), notice, Owed::Speaker(sender)).await?;
    let result = match &landed {
        Landed::NewTurn { .. } => "started a turn".to_string(),
        Landed::JoinedTurn { .. } => "joined its turn".to_string(),
        Landed::Waiting { .. } => "waiting for it".to_string(),
        Landed::Sent => "sent".to_string(),
        Landed::Handed { .. } => "handed to its session".to_string(),
        Landed::Uncertain { why } => format!("acceptance unknown: {why}"),
    };
    let action = Action {
        at,
        machine: to.machine.clone(),
        hand: "message_colleague".to_string(),
        what: to.resident.to_string(),
        result,
    };
    record(node, &from, town, action).await;
    Ok((to, landed))
}

/// Which machine the resident `to` lives on, for words from `from`: the
/// machine `from` asked it on, when it did; this machine, when it lives
/// here; otherwise whichever machine says it lives there.
async fn locate(
    node: &Arc<Node>,
    from: &ResidentId,
    town: Option<&TownName>,
    to: &ResidentId,
) -> Result<MachineName, Failure> {
    let here = node.machine();
    let actions = node.world.resident(town, from).actions();
    let asked = tokio::task::spawn_blocking({
        let to = to.to_string();
        move || asked_on(&actions, &to)
    })
    .await
    .map_err(|error| Failure::new(format!("reading what was asked: {error}")))?;
    if let Some(machine) = asked {
        return Ok(machine);
    }
    if node.residents.about(to).is_some() {
        return Ok(here);
    }
    find(node, to).await
}

/// Which other machine says the resident `to` lives there. One id on
/// several machines is said as it is, never chosen between.
pub async fn find(node: &Arc<Node>, to: &ResidentId) -> Result<MachineName, Failure> {
    let here = node.machine();
    let Some(link) = node.link() else {
        return Err(Failure::new(format!("no resident {to} lives on {here}")));
    };
    let _ = link.refresh().await;
    let (table, presence) = link.members();
    let mut asks = Vec::new();
    let mut offline = Vec::new();
    for member in table.machines().filter(|member| member.name != here) {
        if presence.iter().any(|presence| presence.name == member.name && presence.online) {
            let (link, name, to) = (link.clone(), member.name.clone(), to.clone());
            asks.push(tokio::spawn(async move {
                let found = link.call(&name, RemoteCall::Find { resident: to }).await;
                (name, found)
            }));
        } else {
            offline.push(member.name.to_string());
        }
    }
    let mut found = Vec::new();
    let mut unanswered = Vec::new();
    for ask in asks {
        let Ok((name, answer)) = ask.await else { continue };
        match answer {
            Ok(Ok(RemoteReply::Found { lives_here: true })) => found.push(name),
            Ok(Ok(_)) => {}
            Ok(Err(failure)) => unanswered.push(format!("{name} ({failure})")),
            Err(CallFailure::Offline { name, .. }) => offline.push(name.to_string()),
            Err(failure) => unanswered.push(format!("{name} ({failure})")),
        }
    }
    match found.as_slice() {
        [machine] => Ok(machine.clone()),
        [] => {
            let mut text = format!("no machine that answered has a resident {to}");
            if !offline.is_empty() {
                text.push_str(&format!("; offline: {}", offline.join(", ")));
            }
            if !unanswered.is_empty() {
                text.push_str(&format!("; no answer from: {}", unanswered.join(", ")));
            }
            Err(Failure::new(text))
        }
        several => {
            let names: Vec<String> = several.iter().map(ToString::to_string).collect();
            Err(Failure::new(format!(
                "{to} lives on more than one machine ({}); name the one you mean as {to}@<machine>",
                names.join(", ")
            )))
        }
    }
}

/// A resident of this machine that the user opened left. Its colleagues
/// here that are idle go to sleep, and those working sleep once their turn
/// ends; machines it asked colleagues on are told by letter.
pub fn left(node: &Arc<Node>, resident: ResidentId) {
    let node = node.clone();
    tokio::spawn(async move {
        let address = Address { machine: node.machine(), resident: resident.clone() };
        sleep_idle(&node, &address);
        let Some(town) = node.residents.town_of(&resident) else { return };
        let actions = node.world.resident(town.as_ref(), &resident).actions();
        let machines = tokio::task::spawn_blocking(move || asked_machines(&actions)).await.unwrap_or_default();
        let Some(link) = node.link() else { return };
        for machine in machines.into_iter().filter(|machine| *machine != address.machine) {
            let id = LetterId::try_from(format!("left-{resident}-{}", Timestamp::now().as_millisecond()))
                .expect("a resident id and digits make a letter id");
            if let Err(error) = link.send_letter(&machine, id, Letter::Left { resident: resident.clone() }) {
                node.log.line(format_args!("telling {machine} that {resident} left: {error}"));
            }
        }
    });
}

/// A resident of another machine that asked for colleagues here left.
pub fn departed(node: &Node, asker: Address) {
    node.residents.set_departed(asker.clone(), true);
    sleep_idle(node, &asker);
}

fn sleep_idle(node: &Node, asker: &Address) {
    for colleague in node.residents.colleagues_of(asker) {
        let idle = node.residents.now(&colleague).is_some_and(|now| now.activity == Activity::Idle);
        if idle
            && let Err(error) = node.residents.change_now(&node.world, &colleague, |now| {
                now.activity = Activity::Asleep;
                now.since = Timestamp::now();
            })
        {
            node.log.line(format_args!("recording that {colleague} sleeps: {error}"));
        }
    }
}

/// The machines a resident asked colleagues on, from its `actions`.
fn asked_machines(actions: &std::path::Path) -> Vec<MachineName> {
    let Ok(text) = std::fs::read_to_string(actions) else { return Vec::new() };
    let mut machines: Vec<MachineName> = text
        .lines()
        .filter_map(|line| serde_json::from_str::<Action>(line).ok())
        .filter(|action| action.hand == "ask_colleague")
        .map(|action| action.machine)
        .collect();
    machines.sort();
    machines.dedup();
    machines
}

/// The machine `to` was asked on, from the asker's `actions`.
fn asked_on(actions: &std::path::Path, to: &str) -> Option<MachineName> {
    let text = std::fs::read_to_string(actions).ok()?;
    text.lines()
        .rev()
        .filter_map(|line| serde_json::from_str::<Action>(line).ok())
        .find(|action| action.hand == "ask_colleague" && action.result == to)
        .map(|action| action.machine)
}

async fn record(node: &Arc<Node>, resident: &ResidentId, town: Option<TownName>, action: Action) {
    let (world, resident) = (node.world.clone(), resident.clone());
    let recorded =
        tokio::task::spawn_blocking(move || crate::residents::record_action(&world, &resident, town.as_ref(), &action))
            .await;
    if let Ok(Err(error)) | Err(error) = recorded.map_err(std::io::Error::other) {
        node.log.line(format_args!("recording an action: {error}"));
    }
}
