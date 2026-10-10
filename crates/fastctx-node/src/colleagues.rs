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
use fastctx_types::access::{Acting, Purpose, Written};
use fastctx_types::ipc::Failure;
use fastctx_types::remote::{Letter, RemoteCall, RemoteReply};
use fastctx_types::residents::{
    About, Action, Activity, Delivery, Host, Landed, MessageKind, MessageLine, Notice, Now, Origin, Pending,
    PendingState,
};
use fastctx_types::session::{Speaker, TurnHow};
use fastctx_types::{AccountName, Address, AgentName, MachineName, ResidentId, TownName};
use jiff::Timestamp;

use crate::Node;
use crate::residents::Heard;
use crate::speech::{self, Owed};

pub mod setting;

/// Asks for a colleague for `asker`, a resident of this machine, where the
/// user set its colleagues to live. Returns where the colleague lives.
pub async fn ask(
    node: &Arc<Node>,
    asker: ResidentId,
    town: Option<TownName>,
    label: String,
    prompt: String,
) -> Result<Address, Failure> {
    let here = node.machine();
    let at = Timestamp::now();
    let (about, _) =
        node.residents.about(&asker).ok_or_else(|| Failure::not_run(format!("no resident {asker} lives here")))?;
    if let Origin::Colleague { asked_by, .. } = &about.origin {
        return Err(Failure::not_run(format!(
            "you are a colleague {asked_by} asked for, and a colleague does not ask for colleagues; say what you need in your last message"
        )));
    }
    let lives_on = setting::read(node, &asker)
        .map_err(|error| Failure::not_run(format!("reading whether this conversation may ask for colleagues: {error}")))?
        .ok_or_else(|| {
            Failure::not_run(
                "asking for colleagues is off for this conversation; the user turns it on, and chooses the machine colleagues live on, in the conversation's details",
            )
        })?;
    let home = setting::home(node, &asker)
        .map_err(|error| Failure::not_run(format!("reading which Codex home colleagues use: {error}")))?;
    let colleague = if node.is_here(&lives_on) {
        let address = Address { machine: here.clone(), resident: asker.clone() };
        let id = ask_here(node, address, about.acting.clone(), town.clone(), label.clone(), prompt, home).await?;
        Address { machine: here, resident: id }
    } else {
        let Address { machine, resident } =
            ask_there(node, &about, &lives_on, town.clone(), label.clone(), prompt, home).await?;
        Address { machine, resident }
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

/// Asks `lives_on`, another machine, for a colleague for `asker`: working
/// for those of the asker's accounts that may use the agents there.
async fn ask_there(
    node: &Arc<Node>,
    asker: &About,
    lives_on: &Written,
    town: Option<TownName>,
    label: String,
    prompt: String,
    home: Option<std::path::PathBuf>,
) -> Result<Address, Failure> {
    let set = "the user chooses the machine colleagues live on in this conversation's details";
    let (link, account) = node.speaking(&asker.acting, lives_on, Purpose::Agents).map_err(|failure| {
        Failure::not_run(format!(
            "{lives_on}, where this conversation's colleagues live, cannot be asked: {failure}; {set}"
        ))
    })?;
    let view = link.view();
    let here = node.machine();
    let accounts: Vec<AccountName> = node
        .accounts_on(&link, &asker.acting)
        .into_iter()
        .filter(|other| *other != account && view.admits(&here, other, &lives_on.name, Purpose::Agents).is_ok())
        .collect();
    // A project is of one hub; a colleague asked on another is born
    // outside it.
    let hub = link.membership().id.clone();
    let town = town.filter(|town| {
        crate::ledger::names::place(&node.world, town).ok().flatten().is_some_and(|place| place.hub == hub)
    });
    let call = RemoteCall::Ask { asker: asker.id.clone(), town, label, prompt, accounts, home };
    match link.call(&lives_on.name, &account, Purpose::Agents, call).await {
        Ok(Ok(RemoteReply::Asked { colleague })) => Ok(Address { machine: lives_on.name.clone(), resident: colleague }),
        Ok(Ok(_)) => Err(Failure::new(format!("{lives_on} answered something other than a colleague"))),
        // What went wrong there is said of that machine, not the asker's.
        Ok(Err(failure)) => {
            Err(Failure { message: format!("on {lives_on}: {}", failure.message), not_run: failure.not_run })
        }
        Err(CallFailure::Offline { last_seen, .. }) => Err(Failure::not_run(format!(
            "{lives_on}, where this conversation's colleagues live, is offline{}; {set}",
            last_seen.map(|at| format!(" (since {at})")).unwrap_or_default()
        ))),
        Err(failure) => Err(Failure::new(format!("asking {lives_on}: {failure}"))),
    }
}

/// Starts a colleague on this machine for `asker`, which may live on
/// another, with `prompt` as its first words. Returns its id once its
/// thread exists; the words go in after, as its tools may take a while to
/// start.
pub async fn ask_here(
    node: &Arc<Node>,
    asker: Address,
    acting: Acting,
    town: Option<TownName>,
    label: String,
    prompt: String,
    home: Option<std::path::PathBuf>,
) -> Result<ResidentId, Failure> {
    node.residents.set_departed(asker.clone(), false);
    if let Some(town) = &town {
        crate::town::project::spans_here(node, town).await?;
    }
    // The Codex home it is born in must be one this machine has, so the
    // account the user chose is the one that answers.
    let home = match home {
        None => None,
        Some(dir) => {
            let env = crate::claude::environment()?;
            let user_home = fastctx_platform::user_home().map_err(|error| Failure::new(error.to_string()))?;
            let home = crate::homes::find(node, &env, &user_home, "codex", Some(&dir)).map_err(|failure| {
                Failure::not_run(format!("{}; the user chooses it in the conversation's details", failure.message))
            })?;
            (home.found != fastctx_types::agents::HomeFound::Default).then_some(home.dir)
        }
    };
    let agent = AgentName::try_from("codex").expect("a valid agent name");
    let id = node.residents.new_id(&agent)?;
    let origin = Origin::Colleague { asked_by: asker.clone(), label };
    let _birth = crate::here::born(node, &id, acting.clone(), None);
    let (spec, heard) = thread_spec(node, &id, &origin, town.as_ref(), &acting, None, home.as_deref()).await?;
    let (thread, model) = node.codex.home(home.as_deref())?.start_thread(spec).await.map_err(|error| {
        Failure::new(format!("starting Codex on {}, where colleagues live: {error}", node.machine()))
    })?;
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
        acting,
        works_on: None,
        home,
    };
    let now = Now {
        activity: Activity::Busy,
        since: born_at,
        turn: None,
        owed: Default::default(),
        answering: vec![asker.clone()],
        trouble: None,
        desk: false,
        desk_since: None,
        waiting_for_user: false,
        waiting_since: None,
        title: None,
        last_tool_at: None,
        first_words: None,
        last_said: None,
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
    let answering = node.handoff.call();
    tokio::spawn(async move {
        let Some(_answering) = answering else { return };
        let why = match speech::hand_over(&node, &colleague, &about, &directory, first.clone()).await {
            Ok(
                Landed::NewTurn { .. }
                | Landed::JoinedTurn { .. }
                | Landed::Sent
                | Landed::Handed { .. }
                | Landed::Uncertain { .. },
            ) => return,
            Ok(Landed::Waiting { why, .. }) if why.as_deref() == Some(crate::handoff::WAITING) => return,
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
/// its own name. Also what it is told of its town by that. `home` is the
/// Codex login it is of, none for the default one here; a login lent to
/// another account goes without the hands that work this machine's screen
/// and browser (`codex::lend`).
pub async fn thread_spec(
    node: &Arc<Node>,
    id: &ResidentId,
    origin: &Origin,
    town: Option<&TownName>,
    acting: &Acting,
    works_on: Option<&Written>,
    home: Option<&std::path::Path>,
) -> Result<(ThreadSpec, Heard), Failure> {
    let here = node.machine();
    let cwd = standing(node, town)?;
    let binary = node.fastctx.clone();
    let address = Address { machine: here.clone(), resident: id.clone() };
    let env = vec![
        ("FASTCTX_HOME".to_string(), node.root.to_string_lossy().into_owned()),
        ("FASTCTX_RESIDENT".to_string(), address.tag()),
    ];
    let mut tools = vec!["run"];
    // A colleague's thread is listed under its task; a session the user
    // opens is named by whichever client shows it first, as Codex does.
    let name = match origin {
        Origin::Colleague { label, .. } => Some(label.clone()).filter(|label| !label.trim().is_empty()),
        Origin::User => {
            tools.extend(["ask_colleague", "message_colleague"]);
            None
        }
    };
    let mut config = codex::world_tools(&binary, &env, &tools, fastctx_hands::run::window::BUDGET);
    if works_on.filter(|works_on| !node.is_here(works_on)).is_none() && node.works_here(acting).is_err() {
        let config_home = home.unwrap_or_else(|| node.codex.default().config_home());
        let (servers, plugins) = codex::desktop_hands(config_home).map_err(Failure::new)?;
        codex::lend(&mut config, &servers, &plugins);
    }
    crate::session_options::configure(&mut config, crate::session_options::read(node, id)?);
    let (developer_instructions, heard) = instructions(node, id, town, origin, acting, works_on).await?;
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
/// is, its hands, and its town's record. Also what it is told of its town by
/// that: how far into the ledger, read before the record so nothing written
/// meanwhile goes unsaid.
pub(crate) async fn instructions(
    node: &Arc<Node>,
    id: &ResidentId,
    town: Option<&TownName>,
    origin: &Origin,
    acting: &Acting,
    works_on: Option<&Written>,
) -> Result<(String, Heard), Failure> {
    let heard = heard_now(node, town).map_err(|error| Failure::new(format!("reading the town's ledger: {error}")))?;
    let works_on = works_on.filter(|works_on| !node.is_here(works_on));
    // An agent lent to another account is given the machines it may work
    // on, and never the name of the one it runs on (`world-run.md`, 这里).
    let lent = (works_on.is_none() && node.works_here(acting).is_err()).then(|| crate::world::machines(node, acting).1);
    let words = words(id, &node.machine(), works_on, lent.as_deref(), town, origin);
    let text = match town {
        Some(town) => format!("{words}\n\n{}", crate::town::map::brief(node, town).await?),
        None => words,
    };
    Ok((text, heard))
}

/// `lent` holds the machines it may work on when it is an agent another
/// account lent, which may not work on this machine: none of its scripts
/// starts here, and this machine goes unnamed.
fn words(
    id: &ResidentId,
    machine: &MachineName,
    works_on: Option<&Written>,
    lent: Option<&[String]>,
    town: Option<&TownName>,
    origin: &Origin,
) -> String {
    let place = match (town, origin) {
        (Some(town), _) => format!(
            " You work in the user's project {town}: a fastctx project, which is not a folder or a repository but a named piece of work that can span several machines. Its record follows: what it is for, its machines, and where its code and data are on each."
        ),
        (None, Origin::User) => " When the user names the project to work on, enter it with `fastctx project enter NAME` (`fastctx project list` lists them).".to_string(),
        (None, Origin::Colleague { .. }) => String::new(),
    };
    let working = match origin {
        Origin::Colleague { asked_by, .. } => format!(
            "{} on {} started you, and you work for it. Its request comes next, under a line naming it; messages from anyone else arrive the same way. At the end of each turn, your last message goes word for word to whoever's messages went into that turn, and to no one else; nothing you write before it reaches anyone. Put what you found, or what you need to ask, in that last message.",
            asked_by.resident, asked_by.machine
        ),
        Origin::User => "The user started this session from fastctx and talks to you here.".to_string(),
    };
    // Where it lives and where its shell is, when they differ (`world-run.md`,
    // 这里), or that it has none where it lives (`world-network.md`,
    // 共享机器与借出智能体).
    let on = "`on MACHINE` in the script runs a part of it on another machine";
    let (who, runs, elsewhere) = match (works_on, lent) {
        (Some(works_on), _) => (
            format!("an agent session on {machine}, one of the user's machines connected by fastctx"),
            format!("It runs bash on {works_on}, where you work, and {on}"),
            format!(
                " You work on {works_on}: your shell and the files you work on are there. Your own program runs on {machine}, so its built-in tools act on {machine}, and the working directory and platform your host reports are {machine}'s, not {works_on}'s."
            ),
        ),
        (None, Some(machines)) => (
            "an agent session working on the user's machines, which fastctx connects".to_string(),
            "Each script is a single `on MACHINE`, which runs it there".to_string(),
            format!("\n\n{}", fastctx_hands::lent::opening(machines)),
        ),
        (None, None) => (
            format!("an agent session on {machine}, one of the user's machines connected by fastctx"),
            format!("It runs bash on this machine, and {on}"),
            String::new(),
        ),
    };
    format!(
        "You are {id}, {who}.{elsewhere}{place}\n\n{working}\n\nWork through the `run` tool; it is your only shell, editor and image viewer. {runs}; `fastctx view` shows files, images and documents, and `edit` and `write` change files. Its description says how. Paths are absolute, each the path on its own machine, written like C:/Users/me on Windows. A script still running when its call stops waiting (15 seconds by default) goes on by itself in the background; when it ends you are told automatically, with its exit status and last lines, while you work or after you end your turn."
    )
}

/// The user turns asking for colleagues on for `resident`, on any machine,
/// with where they live, or off.
pub async fn set(
    node: &Arc<Node>,
    resident: ResidentId,
    machine: Option<MachineName>,
    lives_on: Option<Written>,
    home: Option<std::path::PathBuf>,
) -> Result<(), Failure> {
    let machine = match machine {
        Some(machine) => machine,
        None if node.residents.about(&resident).is_some() => node.machine(),
        None => find_for(node, &Acting::Here, &resident).await?,
    };
    if machine == node.machine() {
        return setting::set(node, &resident, lives_on, home);
    }
    let call = RemoteCall::Colleagues { resident, lives_on, home };
    match node.call(&Acting::Here, &(&machine).into(), Purpose::Agents, call).await {
        Ok(Ok(RemoteReply::Set)) => Ok(()),
        Ok(Ok(_)) => Err(Failure::new(format!("{machine} answered something other than the setting"))),
        Ok(Err(failure)) => Err(failure),
        Err(failure) => Err(Failure::new(failure.to_string())),
    }
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
    let acting = crate::around::acting_of(node, &crate::town::caller::Caller::Resident(sender.clone()));
    let notice = Notice::Words { from: Speaker::Resident { address: sender.clone() }, text };
    let landed = speech::tell(node, &acting, &to, speech::new_id("words"), notice, Owed::Speaker(sender)).await?;
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
    find(node, from, to).await
}

/// Which other machine says the resident `to` lives there, of those
/// `from`, a resident here, may look at. One id on several machines is said
/// as it is, never chosen between.
pub async fn find(node: &Arc<Node>, from: &ResidentId, to: &ResidentId) -> Result<MachineName, Failure> {
    let here = node.machine();
    let acting = node.residents.about(from).map(|(about, _)| about.acting).unwrap_or_default();
    find_for(node, &acting, to).await.map_err(|failure| {
        if node.connected_anywhere() { failure } else { Failure::new(format!("no resident {to} lives on {here}")) }
    })
}

/// Which machine `acting` may look at says the resident `to` lives there.
pub async fn find_for(node: &Arc<Node>, acting: &Acting, to: &ResidentId) -> Result<MachineName, Failure> {
    let machines = crate::around::reachable(node, acting, Purpose::Look);
    use crate::around::Heard as Answer;
    let heard = crate::around::ask_each(machines, |_| RemoteCall::Find { resident: to.clone() }).await;
    let mut found = Vec::new();
    let mut offline = Vec::new();
    let mut unanswered = Vec::new();
    for (name, heard) in heard {
        match heard {
            Answer::Answered(RemoteReply::Found { lives_here: true }) => found.push(name),
            Answer::Answered(_) => {}
            Answer::Offline(_) => offline.push(name.to_string()),
            Answer::NoAnswer(reason) => unanswered.push(format!("{name} ({reason})")),
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
        let acting = node.residents.about(&resident).map(|(about, _)| about.acting).unwrap_or_default();
        let actions = node.world.resident(town.as_ref(), &resident).actions();
        let machines = tokio::task::spawn_blocking(move || asked_machines(&actions)).await.unwrap_or_default();
        for machine in machines.into_iter().filter(|machine| *machine != address.machine) {
            let id = speech::fixed_id("left", &[resident.as_str(), &Timestamp::now().as_millisecond().to_string()]);
            let letter = Letter::Left { resident: resident.clone() };
            if let Err(error) = crate::letters::send(&node, &acting, &Written::from(&machine), id, letter) {
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
