//! Asking for a colleague and speaking to one (`world-colleagues.md`, 两个
//! 工具).
//!
//! A colleague is a Codex resident the World starts, on the asker's machine
//! or another, as a thread of that machine's app-server. The asker's words
//! become its goal and first turn; the goal's report returns to the asker. Asking is
//! recorded as one of the asker's actions: where the colleague lives, and
//! its id; speaking to it later starts from that record.


mod native;
pub(crate) mod plan;
pub(crate) mod stopping;
pub(crate) mod tree;

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
use fastctx_types::{Address, AgentName, MachineName, ResidentId, TownName};
use jiff::Timestamp;

use crate::Node;
use crate::collab::Whose;
use crate::residents::Heard;
use crate::speech::{self, Owed};

/// Starts the named callee configured by an account the caller serves.
/// Returns where the colleague lives.
pub async fn ask(
    node: &Arc<Node>,
    asker: ResidentId,
    town: Option<TownName>,
    label: String,
    prompt: String,
    callee: Option<String>,
    login: Option<&fastctx_types::collab::Caller>,
) -> Result<Address, Failure> {
    let at = Timestamp::now();
    let turn = node.residents.now(&asker).and_then(|now| now.turn);
    let (about, _) =
        node.residents.about(&asker).ok_or_else(|| Failure::not_run(format!("no resident {asker} lives here")))?;
    let choices = crate::collab::require(node, &asker, login)?;
    let mut matching: Vec<_> = choices
        .candidates
        .into_iter()
        .filter(|choice| callee.as_ref().is_none_or(|name| *name == choice.shown))
        .collect();
    let chosen = match matching.len() {
        1 => matching.pop().expect("one callee"),
        0 => {
            return Err(Failure::not_run(
                "the named callee is not configured for this caller; the user changes it on the Model collaboration page",
            ));
        }
        _ => {
            return Err(Failure::not_run(
                "choose a callee by its offered name; the user configures them on the Model collaboration page",
            ));
        }
    };
    let (colleague, account) = match chosen.whose {
        Whose::Account { link, account } => {
            let request = Request { about: &about, turn: turn.clone(), town: town.as_ref(), label: label.clone(), prompt };
            let colleague = ask_for_account(node, &link, &account, chosen.started, request).await?;
            (colleague, Some(fastctx_types::access::HubAccount { hub: link.membership().id.clone(), account }))
        }
        // This computer's own settings, in no network: the colleague would be
        // born here, but no account keeps the goal it works toward.
        Whose::Computer => return Err(Failure::not_run(fastctx_types::collab_prompt::GOAL_NEEDS_NETWORK)),
    };
    let action = Action {
        turn, at,
        machine: colleague.machine.clone(),
        hand: "ask_colleague".to_string(),
        what: label,
        result: colleague.resident.to_string(),
    };
    if let Some(account) = &account {
        crate::collab::keep_asked(node, &asker, &colleague, account)?;
    }
    record(node, &asker, action).await;
    Ok(colleague)
}

/// What a resident asks of a colleague.
struct Request<'a> {
    about: &'a About,
    turn: Option<String>,
    town: Option<&'a TownName>,
    label: String,
    prompt: String,
}

/// Starts a callee an account configured, on whichever of its machines the
/// callee lives, once the account may still use its login and the project.
async fn ask_for_account(
    node: &Arc<Node>,
    link: &Arc<fastctx_net::link::Link>,
    account: &fastctx_types::AccountName,
    started: fastctx_types::collab::Started,
    request: Request<'_>,
) -> Result<Address, Failure> {
    let machine = started.callee.login.machine.clone();
    let view = link.view();
    let call_town = node.town_across(link, request.town)?;
    if let Some(town) = &call_town {
        let allowed = view
            .projects
            .iter()
            .find(|project| project.name == *town)
            .is_some_and(|project| view.in_project(account, &project.town));
        if !allowed {
            return Err(Failure::not_run(format!("the account {account} may not use the project {town}")));
        }
    }
    if !view.may_use_login(account, &machine, &started.callee.login.agent, started.callee.login.home.as_deref()) {
        return Err(Failure::not_run(format!(
            "the account {account} can no longer use this callee's login; the user changes it on the Model collaboration page"
        )));
    }
    let here = node.machine();
    if link.view().is_me(&machine) {
        let acting = Acting::one(link.membership().id.clone(), account.clone());
        let address = Address { machine: here.clone(), resident: request.about.id.clone() };
        let id = ask_here(node, Requester { address, turn: request.turn }, acting, call_town, request.label, request.prompt, started).await?;
        return Ok(Address { machine: here, resident: id });
    }
    let lives_on = node.written(link, &machine);
    // A machine before the colleague goals answers Ask with a colleague of
    // one turn and no goal.
    match link.call(&lives_on.name, account, Purpose::Look, RemoteCall::Facts).await {
        Ok(Ok(RemoteReply::Facts(facts))) if facts.goal_colleagues => {}
        Ok(Ok(_)) => return Err(Failure::not_run(fastctx_types::collab_prompt::colleague_host_old(&lives_on))),
        Ok(Err(failure)) => return Err(Failure { message: format!("on {lives_on}: {}", failure.message), ..failure }),
        // Not reached: the ask below says how, offline included.
        Err(_) => {}
    }
    let call = RemoteCall::Ask {
        asker: request.about.id.clone(),
        requester_turn: request.turn,
        town: call_town,
        label: request.label,
        prompt: request.prompt,
        started,
    };
    match link.call(&lives_on.name, account, Purpose::Agents, call).await {
        Ok(Ok(RemoteReply::Asked { colleague })) => Ok(Address { machine: lives_on.name.clone(), resident: colleague }),
        Ok(Ok(_)) => Err(Failure::new(format!("{lives_on} answered something other than a colleague"))),
        // The far side says where it is changed when that is what to do.
        Ok(Err(failure)) => Err(Failure { message: format!("on {lives_on}: {}", failure.message), ..failure }),
        Err(CallFailure::Offline { last_seen, .. }) => Err(Failure::not_run(format!(
            "{lives_on}, where this callee lives, is offline{}; the user changes it on the Model collaboration page",
            last_seen.map(|at| format!(" (since {at})")).unwrap_or_default()
        ))),
        Err(failure) => Err(Failure::new(format!(
            "asking {lives_on}: {failure}; the user changes it on the Model collaboration page"
        ))),
    }
}

/// A request keeps its sender's turn while another host is being started.
pub(crate) struct Requester {
    pub address: Address,
    pub turn: Option<String>,
}

/// Starts a colleague on this machine for `asker`, which may live on
/// another, with `prompt` as its first words. Returns its id once its
/// thread exists; the words go in after, as its tools may take a while to
/// start.
pub async fn ask_here(
    node: &Arc<Node>,
    requester: Requester,
    acting: Acting,
    town: Option<TownName>,
    label: String,
    prompt: String,
    started: fastctx_types::collab::Started,
) -> Result<ResidentId, Failure> {
    crate::goals::colleagues::ready(node)?;
    let Requester { address: asker, turn: requester_turn } = requester;
    node.residents
        .set_departed(&node.world, &asker, false, None)
        .map_err(|error| Failure::not_run(format!("recording that {asker} returned: {error}")))?;
    if let Some(town) = &town {
        crate::town::access::ready(node, &acting, town).await?;
    }
    // The Codex home it is born in must be one this machine has, so the
    // account the user chose is the one that answers.
    if !node.is_me(&started.callee.login.machine) {
        return Err(Failure::not_run("the configured callee lives on another machine"));
    }
    let home = crate::homes::listed(node, "codex", started.callee.login.home.as_deref()).map_err(|failure| {
        Failure::not_run(format!("{}; the user changes it on the Model collaboration page", failure.message))
    })?;
    let agent = AgentName::try_from("codex").expect("a valid agent name");
    let id = node.residents.new_id(&agent)?;
    node.codex.home(home.as_deref())?.require_login().await.map_err(|error| {
        Failure::not_run(format!(
            "this callee's Codex login is unavailable: {error}; the user changes it on the Model collaboration page"
        ))
    })?;
    crate::collab::keep(node, &id, &started)?;
    crate::session_options::keep(
        node,
        &id,
        &fastctx_types::agents::SessionOptions {
            mcp_off: Default::default(),
            model: started.callee.model.clone(),
            reasoning_effort: started.callee.effort.clone(),
        },
    )?;
    let origin = Origin::Colleague { asked_by: asker.clone(), label };
    crate::layers::may_start(node, &acting)?;
    let _birth = crate::here::born(node, &id, acting.clone(), &origin);
    let (spec, heard) = thread_spec(node, &id, &origin, town.as_ref(), &acting, home.as_deref()).await?;
    // Until it is claimed below, reports of threads nobody claims are kept.
    let _starting = crate::codex::starting(node);
    let (thread, model) = node
        .codex
        .session(home.as_deref(), crate::assists::host::kept(node, &id)?.unwrap_or(false))?
        .start_thread(spec)
        .await
        .map_err(|error| {
            Failure::new(format!("starting Codex on {}, where colleagues live: {error}", node.machine()))
        })?;
    let born_at = Timestamp::now();
    let about = About {
        side: false,
        forked_from: None,
        mcp_off: Default::default(),
        id: id.clone(),
        host: Host { agent, client: "codex app-server".to_string(), version: String::new() },
        machine: node.machine(),
        born_at,
        origin,
        session: Some(thread.clone()),
        world_started: true,
            native_resumed: false,
        model,
        acting,
        focus: Default::default(),
        home,
        callee: Some(started.callee.name.clone()),
    };
    let now = Now {
        activity: Activity::Busy,
        since: born_at,
        turn: None,
        last_turn: None,
        owed: Default::default(),
        answering: Vec::new(),
        trouble: None,
        desk: false,
        desk_since: None,
        waiting_for_user: false,
        waiting_since: None,
        title: None,
        last_tool_at: None,
        first_words: None,
        last_said: None,
        last_compacted: false,
        last_heard_user: None,
        asked: None,
        told: None,
    };
    if let Some(town) = &town {
        crate::town::access::check(node, &about.acting, town)?;
    }
    node.residents
        .create(&node.world, about.clone(), town.clone(), now)
        .map_err(|error| Failure::new(format!("recording the colleague: {error}")))?;
    crate::context::instructed(node, &id, &heard).map_err(|error| Failure::new(error.to_string()))?;
    if let Err(failure) = crate::goals::colleagues::open(node, &id, &asker, &prompt).await {
        node.residents
            .change_now(&node.world, &id, |now| {
                now.activity = Activity::Idle;
                now.trouble =
                    Some(fastctx_types::residents::Trouble { at: Timestamp::now(), text: failure.message.clone() });
            })
            .map_err(|error| Failure::new(error.to_string()))?;
        return Err(Failure::new(fastctx_types::collab_prompt::goal_failed(&id, failure)));
    }
    node.residents.set_instructed(&id, heard);
    crate::codex::claim(node, &thread, &id).await;
    let directory = node.world.conversation(&id);
    let asked = MessageLine { at: born_at, what: MessageKind::Asked { from: asker.clone(), text: prompt.clone(), requester_turn } };
    if let Err(error) = crate::residents::append_message(&directory, &asked) {
        node.log.line(format_args!("recording what {id} was asked: {error}"));
    }
    // The request is kept before the asker hears of the colleague: a node
    // that stops before it goes in finds it, and puts it in then.
    let first = Pending {
        delivery: Delivery {
            id: speech::new_id("words"),
            at: born_at,
            notice: Notice::Words {
                from: Speaker::Resident { address: asker.clone() },
                text: prompt,
                goal_report: None,
                goal_question: None,
            },
        },
        readers: Vec::new(),
        state: PendingState::Waiting,
        working: false,
        session: None,
        since: None,
    };
    if let Err(error) = crate::inbox::keep(&directory, &first) {
        node.residents
            .change_now(&node.world, &id, |now| {
                now.activity = Activity::Idle;
                now.trouble = Some(fastctx_types::residents::Trouble { at: Timestamp::now(), text: error.to_string() });
            })
            .map_err(|error| Failure::new(error.to_string()))?;
        return Err(Failure::new(fastctx_types::collab_prompt::first_request_failed(&id, error)));
    }
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
            // Kept on its way, as for room in its login or a release being
            // put in: it goes in when it may.
            Ok(Landed::Waiting { answered: true, .. }) => return,
            Ok(Landed::Waiting { why, .. }) => why.unwrap_or_else(|| "its session could not be reached".to_string()),
            Err(failure) => failure.message,
        };
        // Its first turn never began; the goal classifies this failure and decides what follows.
        crate::turns::unsettled(&node, &colleague, &directory, first, TurnHow::Failed, why.clone()).await;
        crate::turns::ended(
            &node,
            &colleague,
            "",
            fastctx_types::session::TurnEnd {
                facts: None,
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
/// Codex login it is of, none for the default one here; a login assisting
/// another account goes without the hands that work this machine's screen
/// and browser, and has its own instruction and extension home.
pub async fn thread_spec(
    node: &Arc<Node>,
    id: &ResidentId,
    origin: &Origin,
    town: Option<&TownName>,
    acting: &Acting,
    home: Option<&std::path::Path>,
) -> Result<(ThreadSpec, Heard), Failure> {
    let here = node.machine();
    crate::profile::session::ready(node, acting).await?;
    let assisting = crate::assists::host::starting(node, id, acting)?;
    let host = node.codex.session(home, assisting)?;
    let cwd = match host.assisted_cwd() {
        Some(cwd) => cwd.to_path_buf(),
        None => standing(node, town)?,
    };
    let binary = node.fastctx.clone();
    let address = Address { machine: here.clone(), resident: id.clone() };
    let env = vec![
        ("FASTCTX_HOME".to_string(), node.root.to_string_lossy().into_owned()),
        ("FASTCTX_RESIDENT".to_string(), address.tag()),
        ("FASTCTX_AGENT_HOME".to_string(), home.map(|home| home.to_string_lossy().into_owned()).unwrap_or_default()),
    ];
    let mut tools = vec!["run"];
    // A colleague's thread is listed under its task; a session the user
    // opens is named by whichever client shows it first, as Codex does.
    let name = match origin {
        Origin::Colleague { label, .. } => Some(label.clone()).filter(|label| !label.trim().is_empty()),
        Origin::User => {
            tools.extend(["ask_colleague", "message_colleague", fastctx_types::voice::TOOL]);
            None
        }
    };
    let mut config = codex::world_tools(&binary, &env, &tools, fastctx_hands::run::window::BUDGET);
    let started = crate::collab::started(node, id)?;
    if !assisting && started.as_ref().is_some_and(|started| !started.callee.screen) {
        let config_home = home.unwrap_or_else(|| node.codex.default().config_home());
        let (servers, plugins) = codex::desktop_hands(config_home).map_err(Failure::new)?;
        codex::without_desktop(&mut config, &servers, &plugins);
    }
    let user_mcp = crate::profile::session::codex(node, id, acting, &mut config)?;
    crate::session_options::configure(&mut config, crate::session_options::read(node, id)?);
    if assisting {
        codex::assisted::configure_thread(&mut config);
    }
    let (mut developer_instructions, heard) = instructions(node, id, town, origin, acting, true).await?;

    if let Some(started) = started.filter(|started| !started.rules.is_empty()) {
        developer_instructions.push_str("\n\n");
        developer_instructions.push_str(&started.rules);
    }
    // What Code Mode needs before a long answer is seen whole, last, as in
    // the guidance file: a thread started here may have no such file.
    developer_instructions.push('\n');
    developer_instructions.push_str(fastctx_install::briefing::CODEX_EXEC);
    Ok((
        ThreadSpec {
            native_resume: false,            user_mcp,
            owner: Address { machine: node.machine(), resident: id.clone() },
            cwd,
            developer_instructions,
            config,
            name,
            choices: node.world.conversation(id).choices(),
        },
        heard,
    ))
}

/// How far into its town's ledger a resident is told of it, as of now.
pub fn heard_now(node: &Node, town: Option<&TownName>) -> std::io::Result<Heard> {
    let ledger = match town {
        Some(town) => crate::ledger::seq(&node.world, town)?,
        None => 0,
    };
    Ok(Heard { scratch: None, context: None, user_prompt: None, town: town.cloned(), ledger })
}

/// The town's bookkeeping directory, where a resident the World starts
/// stands: it means nothing to the resident, but groups a town's sessions
/// together in the hosts' own lists (`world-town.md`).
pub(crate) fn standing(node: &Node, town: Option<&TownName>) -> Result<PathBuf, Failure> {
    let directory = node.world.standing(town);
    fastctx_platform::ensure_private_dir(&directory)
        .map_err(|error| Failure::new(format!("making {}: {error}", directory.display())))?;
    Ok(directory)
}

/// What a resident the World starts is told of itself, as Codex's developer
/// instructions or Claude's system prompt, which outlive compaction: who it
/// is, its hands, and its town's record. Also what it is told of its town by
/// that: how far into the ledger, read before the record so nothing written
/// meanwhile goes unsaid.
///
/// A Claude session's plugin gives it its town's record, as it is each time
/// Claude Code reads its context (`town::instructions`); for it, `record`
/// is false and the instructions leave the record out.
pub(crate) async fn instructions(
    node: &Arc<Node>,
    id: &ResidentId,
    town: Option<&TownName>,
    origin: &Origin,
    acting: &Acting,
    record: bool,
) -> Result<(String, Heard), Failure> {
    let mut heard =
        heard_now(node, town).map_err(|error| Failure::new(format!("reading the town's ledger: {error}")))?;
    let words = words(id, town, origin, record);
    let (context, receipt) = crate::context::prepare(node, id, town, origin, acting).await?;
    heard.context = record.then_some(receipt);
    let text = if record && !context.is_empty() { format!("{words}\n\n{context}") } else { words };
    Ok((text, heard))
}

fn words(id: &ResidentId, town: Option<&TownName>, origin: &Origin, record: bool) -> String {
    let given = if record { "Its record follows" } else { "Its record comes with your context" };
    let place = match (town, origin) {
        (Some(town), _) => format!(
            " You work in the user's project {town}: a fastctx project, which is not a folder or a repository but a named piece of work that can span several machines. {given}: the user's instructions for all work in it, which you follow, its machines, and where its code and data are on each."
        ),
        (None, Origin::User) => " When the user names the project to work on, enter it with `fastctx project enter NAME` (`fastctx project list` lists them).".to_string(),
        (None, Origin::Colleague { .. }) => String::new(),
    };
    let working = match origin {
        Origin::Colleague { asked_by, .. } => format!(
            "{} asked you to do this work.\n\n{}",
            asked_by.resident,
            fastctx_types::collab_prompt::COLLEAGUE_RULES
        ),
        Origin::User => "The user started this session from fastctx and talks to you here.".to_string(),
    };
    // Where scripts run is said by the machines `run`'s description lists,
    // never by where the session lives (`world-run.md`, 脚本在哪台机器上跑).
    let runs =
        "It runs a bash script on one of the machines its description lists, the script written as one `on MACHINE`";
    format!(
        "You are {id}, an agent session.{place}\n\n{working}\n\nWork through the `run` tool; it is your only shell, editor and image viewer. {runs}; `fastctx view` shows files, images and documents, and `edit` and `write` change files. Its description says how. Paths are absolute, each the path on its own machine, written like C:/Users/me on Windows. A script still running when its call stops waiting (15 seconds by default) goes on by itself in the background; when it ends you are told automatically, with its exit status and last lines, while you work or after you end your turn."
    )
}

/// Words from `from`, a resident of this machine, to `to`, on any machine.
pub async fn message(
    node: &Arc<Node>,
    from: ResidentId,
    to: ResidentId,
    to_machine: Option<Written>,
    text: String,
    login: Option<&fastctx_types::collab::Caller>,
) -> Result<(Address, Landed), Failure> {
    let choices = crate::collab::require(node, &from, login)?;
    let here = node.machine();
    let at = Timestamp::now();
    let turn = node.residents.now(&from).and_then(|now| now.turn);
    let machine = match to_machine {
        Some(machine) => machine,
        None => locate(node, &from, &to).await?,
    };
    let to = Address { machine: machine.name.clone(), resident: to };
    let sender = Address { machine: here, resident: from.clone() };
    let acting = match crate::collab::asked_account(node, &from, &to)? {
        Some(account) if node.serves(&choices.acting, &account.hub, &account.account) => {
            Acting::one(account.hub, account.account)
        }
        Some(_) => {
            return Err(Failure::not_run(
                "model collaboration no longer enables this caller for the colleague's account; the user changes it on the Model collaboration page",
            ));
        }
        None => choices.acting,
    };
    if node.is_here(&machine) {
        let (recipient, _) = node
            .residents
            .about(&to.resident)
            .ok_or_else(|| Failure::not_run(format!("no resident {} lives here", to.resident)))?;
        // A colleague this computer's own settings started works for its
        // user, as the sessions those settings let ask do.
        let own = matches!((&acting, &recipient.acting), (Acting::Here, Acting::Here));
        let shares = own
            || node.links().iter().any(|link| {
                node.accounts_on(link, &acting)
                    .iter()
                    .any(|account| node.serves(&recipient.acting, &link.membership().id, account))
            });
        if !shares {
            return Err(Failure::not_run(
                "this colleague works for none of the accounts that enable collaboration for this caller",
            ));
        }
    }
    let notice = Notice::Words {
        from: Speaker::Resident { address: sender.clone() },
        text,
        goal_report: None,
        goal_question: None,
    };
    let landed =
        speech::tell_at(node, &acting, &to, &machine, speech::new_id("words"), notice, Owed::Speaker(sender)).await?;
    let result = match &landed {
        Landed::NewTurn { .. } => "started a turn".to_string(),
        Landed::JoinedTurn { .. } => "joined its turn".to_string(),
        Landed::Waiting { .. } => "waiting for it".to_string(),
        Landed::Sent => "sent".to_string(),
        Landed::Handed { .. } => "handed to its session".to_string(),
        Landed::Uncertain { why } => format!("acceptance unknown: {why}"),
    };
    let action = Action {
        turn, at,
        machine: to.machine.clone(),
        hand: "message_colleague".to_string(),
        what: to.resident.to_string(),
        result,
    };
    record(node, &from, action).await;
    Ok((to, landed))
}

/// Which machine the resident `to` lives on, for words from `from`: the
/// machine `from` asked it on, when it did; this machine, when it lives
/// here; otherwise whichever machine says it lives there.
async fn locate(node: &Arc<Node>, from: &ResidentId, to: &ResidentId) -> Result<Written, Failure> {
    let here = node.machine();
    let actions = node.world.conversation(from).actions();
    let asked = tokio::task::spawn_blocking({
        let to = to.to_string();
        move || asked_on(&actions, &to)
    })
    .await
    .map_err(|error| Failure::new(format!("reading what was asked: {error}")))?;
    if let Some(machine) = asked {
        return Ok(machine.into());
    }
    if node.residents.about(to).is_some() {
        return Ok(here.into());
    }
    find(node, from, to).await
}

/// Which other machine says the resident `to` lives there, of those
/// `from`, a resident here, may look at. One id on several machines is said
/// as it is, never chosen between.
pub async fn find(node: &Arc<Node>, from: &ResidentId, to: &ResidentId) -> Result<Written, Failure> {
    let here = node.machine();
    let acting = node.residents.about(from).map(|(about, _)| about.acting).unwrap_or_default();
    find_for(node, &acting, to).await.map_err(|failure| {
        if node.connected_anywhere() { failure } else { Failure::new(format!("no resident {to} lives on {here}")) }
    })
}

/// Which machine `acting` may look at says the resident `to` lives there.
pub async fn find_for(node: &Arc<Node>, acting: &Acting, to: &ResidentId) -> Result<Written, Failure> {
    let machines = crate::around::reachable_each(node, acting, Purpose::Look);
    let heard = crate::around::ask_each(machines, |_| RemoteCall::Find { resident: to.clone() }).await;
    let mut found = Vec::new();
    let mut unanswered = Vec::new();
    for answers in fastctx_net::around::answers::collect(heard) {
        let (source, lives_here, missing) = answers.find();
        if lives_here {
            found.push(source.written);
        }
        unanswered.extend(missing);
    }
    // One machine says it lives there: that is where it is, whoever else did
    // not answer. A resident's id is its own; a second one of the same id
    // behind an account that did not answer is not a reason to refuse the
    // one found. Only when none is found does who did not answer matter.
    let names = found.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ");
    match found.as_slice() {
        [machine] => Ok(machine.clone()),
        [] if unanswered.is_empty() => Err(Failure::new(format!("no machine that answered has a resident {to}"))),
        [] => Err(Failure::new(format!(
            "no machine that answered has a resident {to}; not answered: {}",
            unanswered.join("; ")
        ))),
        _ => Err(Failure::new(format!(
            "{to} lives on more than one machine ({names}); name the one you mean as {to}@<machine>"
        ))),
    }
}

/// A resident of this machine that the user opened left. Its colleagues
/// here that are idle go to sleep, and those working sleep once their turn
/// ends; machines it asked colleagues on are told by letter.
pub fn left(node: &Arc<Node>, resident: ResidentId) {
    let node = node.clone();
    tokio::spawn(async move {
        let address = Address { machine: node.machine(), resident: resident.clone() };
        if let Err(error) = node.residents.sleep_colleagues(&node.world, &address) {
            node.log.line(format_args!("putting {address}'s idle colleagues to sleep: {error}"));
        }
        let Some((about, _)) = node.residents.about(&resident) else { return };
        let acting = about.acting;
        let actions = node.world.conversation(&resident).actions();
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
pub fn departed(node: &Node, asker: Address, letter: (&str, &crate::letters::Identity)) -> std::io::Result<()> {
    node.residents.set_departed(&node.world, &asker, true, Some(letter))
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

async fn record(node: &Arc<Node>, resident: &ResidentId, action: Action) {
    let (world, resident) = (node.world.clone(), resident.clone());
    let recorded =
        tokio::task::spawn_blocking(move || crate::residents::record_action(&world, &resident, &action)).await;
    if let Ok(Err(error)) | Err(error) = recorded.map_err(std::io::Error::other) {
        node.log.line(format_args!("recording an action: {error}"));
    }
    node.shown_changed();
}
