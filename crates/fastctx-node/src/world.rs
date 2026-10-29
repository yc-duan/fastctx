//! This computer among others: its key, its logins on hubs, and its link
//! to each (`world-network.md`).
//!
//! A computer logged in to no hub is a World of one: it has no link, and
//! everything else works the same. Logging in makes its key if it has none,
//! records the membership under `world/net/<hub id>/` and starts the link;
//! logging out of its last account on a hub ends that membership.

use std::io;
use std::net::SocketAddr;
use std::sync::{Arc, Weak};

use fastctx_net::directory::Kind;
use fastctx_net::ids::{CertPin, HubId};
use fastctx_net::link::{BoxFuture, Caller, Link, Membership, NetDir, Status};
use fastctx_net::login::{Device, Joined};
use fastctx_net::message::LetterId;
use fastctx_net::sign::SecretKey;
use fastctx_net::wire::Change;
use fastctx_types::access::{Acting, HubAccount, Purpose};
use fastctx_types::accounts::{AccountChange, AccountsReply};
use fastctx_types::ipc::Failure;
use fastctx_types::layout::WorldDir;
use fastctx_types::remote::{Letter, RemoteCall, RemoteReply, RemoteResult};
use fastctx_types::residents::{Delivery, Notice};
use fastctx_types::session::Speaker;
use fastctx_types::world::MachineFacts;
use fastctx_types::{AccountName, Address, HubName, MachineName, Reply};
use jiff::Timestamp;

use crate::Node;
use crate::speech::Owed;

/// Where the memberships of this computer are kept, one directory per hub.
fn hubs(world: &WorldDir) -> std::path::PathBuf {
    world.net()
}

/// The file in a hub's directory that says this computer is leaving it,
/// naming the account it logged out of.
const LEAVING: &str = "leaving";

fn net_dir(world: &WorldDir, hub: &HubId) -> NetDir {
    NetDir::new(hub_dir(world, hub))
}

/// Where this computer keeps what it has of the hub `hub`.
pub(crate) fn hub_dir(world: &WorldDir, hub: &HubId) -> std::path::PathBuf {
    hubs(world).join(hub.as_str())
}

/// The memberships this computer keeps, one per hub. The network's third
/// version kept one, in `world/net/` itself; it is moved under its hub's
/// id the first time this runs, its hub's name known once it connects.
pub fn load(world: &WorldDir) -> io::Result<Vec<Membership>> {
    migrate(world)?;
    let mut memberships = Vec::new();
    let entries = match std::fs::read_dir(hubs(world)) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(memberships),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let path = entry?.path();
        let Some(id) =
            path.file_name().and_then(|name| name.to_str()).and_then(|name| HubId::try_from(name.to_string()).ok())
        else {
            continue;
        };
        // A computer that began to leave this network and stopped before
        // it was done leaves it now: what it told the hub was the last word
        // (`specs/world-concurrency.md`, 多步的事).
        if path.join(LEAVING).exists() {
            let aside = world.path().join(format!("net-left-{}-{}", id, Timestamp::now().strftime("%Y%m%dT%H%M%SZ")));
            std::fs::rename(&path, &aside)?;
            continue;
        }
        match fastctx_net::link::state::read::<Membership>(&path.join("membership.json")) {
            Ok(membership) if membership.id == id => memberships.push(membership),
            Ok(membership) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{} holds the membership of {}", path.display(), membership.id),
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(memberships)
}

/// The third version's membership, as it was written.
#[derive(serde::Deserialize)]
struct ThirdVersion {
    world: HubId,
    name: MachineName,
    hub: Vec<SocketAddr>,
    cert: CertPin,
}

/// Moves a third-version membership and its records under the hub's id.
fn migrate(world: &WorldDir) -> io::Result<()> {
    let net = hubs(world);
    let old = net.join("membership.json");
    let membership: ThirdVersion = match fastctx_net::link::state::read(&old) {
        Ok(membership) => membership,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let target = net.join(membership.world.as_str());
    fastctx_platform::ensure_private_dir(&target)?;
    for name in ["counter.json", "letters.json", "outbox", "received", "failures"] {
        let from = net.join(name);
        if from.exists() {
            std::fs::rename(&from, target.join(name))?;
        }
    }
    // What the third version kept of the members, its calls taken and its
    // sealing, means nothing to the fourth.
    let aside = world.path().join("net-third-version");
    fastctx_platform::ensure_private_dir(&aside)?;
    for name in ["members.json", "calls.json"] {
        let from = net.join(name);
        if from.exists() {
            std::fs::rename(&from, aside.join(name))?;
        }
    }
    let fourth = Membership {
        id: membership.world,
        // The hub says its name in its welcome, and the link records it.
        hub_name: HubName::try_from("hub").expect("a valid hub name"),
        name: membership.name,
        addresses: membership.hub,
        cert: membership.cert,
        kind: Kind::Computer,
        relays: Vec::new(),
    };
    fastctx_net::link::state::write(&target.join("membership.json"), &fourth)?;
    std::fs::rename(&old, aside.join("membership.json"))
}

/// This computer's key, made now if it has none.
pub fn key(world: &WorldDir) -> io::Result<SecretKey> {
    crate::ledger::own_key(world)
}

/// Starts the link of a membership.
pub fn start(node: &Arc<Node>, membership: Membership) -> io::Result<Arc<Link>> {
    let key = key(&node.world)?;
    let hub = membership.id.clone();
    let net = net_dir(&node.world, &hub).with_database(node.database.clone());
    // A network dissolved as this computer last ran, before it was left, is
    // left now: its hub may have retired since.
    let dissolved = fastctx_net::link::state::read_or::<Option<fastctx_net::link::state::Ended>>(&net.ended(), None)
        .ok()
        .flatten()
        .is_some_and(|ended| ended.dissolved);
    let handler = Arc::new(Handler { node: Arc::downgrade(node), hub: hub.clone() });
    let link = Arc::new(Link::start(net, membership, key, handler)?);
    if let Some(old) = node.links.write().expect("never poisoned").insert(hub.clone(), link.clone()) {
        old.stop();
    }
    // What the link said of its ways before it was among the links was not
    // seen in them.
    node.ways_changed();
    crate::updates::report(node);
    if dissolved {
        let node = node.clone();
        tokio::spawn(async move {
            if let Err(failure) = left_dissolved(&node, &hub).await {
                node.log.line(format_args!("leaving the dissolved network: {failure}"));
            }
        });
    }
    Ok(link)
}

/// Logs this computer in to `account` on the hub at `hub` with `password`.
pub async fn login(
    node: &Arc<Node>,
    hub: Vec<SocketAddr>,
    account: AccountName,
    password: String,
    register: bool,
    name: Option<MachineName>,
) -> Result<Reply, Failure> {
    let key = own_key(node).await?;
    let name = asked_name(node, name)?;
    let device = Device { name, key: &key, kind: Kind::Computer, register };
    let joined = fastctx_net::login::with_password(&hub, &account, &password, device).await.map_err(Failure::new)?;
    logged_in(node, joined).await
}

/// Logs this computer in with an enrollment token. One for a managed
/// machine is used only by a World that may become one (`managed::unfit`).
pub async fn login_with_token(node: &Arc<Node>, token: &str, name: Option<MachineName>) -> Result<Reply, Failure> {
    let token = fastctx_net::token::Token::parse(token).map_err(Failure::new)?;
    let key = own_key(node).await?;
    // Asked as this computer, so a first try cut off is finished again.
    if let Some(peeked) = fastctx_net::login::peek(&token, Some(&key.public())).await.map_err(Failure::new)?
        && peeked.managed
        && let Some(why) = crate::managed::unfit(node, &peeked.hub)
    {
        return Err(Failure::not_run(why.to_string()));
    }
    let name = asked_name(node, name)?;
    // What the token is for, registering or not, a managed machine or not,
    // the hub keeps with it.
    let device = Device { name, key: &key, kind: Kind::Computer, register: true };
    let joined = fastctx_net::login::with_token(&token, device).await.map_err(Failure::new)?;
    logged_in(node, joined).await
}

/// What `token` lets this computer into, asked of its hub without using
/// it; none from a hub that says nothing of a token before it is used.
pub async fn peek_token(node: &Arc<Node>, token: &str) -> Result<Option<fastctx_types::accounts::TokenPeek>, Failure> {
    let token = fastctx_net::token::Token::parse(token.trim()).map_err(Failure::new)?;
    let key = own_key(node).await?;
    let Some(peeked) = fastctx_net::login::peek(&token, Some(&key.public())).await.map_err(Failure::new)? else {
        return Ok(None);
    };
    let refused = if peeked.managed { crate::managed::unfit(node, &peeked.hub) } else { None };
    Ok(Some(fastctx_types::accounts::TokenPeek {
        hub: peeked.hub_name,
        account: peeked.account,
        managed: peeked.managed,
        label: peeked.label,
        expires: peeked.expires,
        refused,
    }))
}

/// Opens `account` on a hub that lets anyone, then logs in to it.
pub async fn open_account(
    node: &Arc<Node>,
    hub: Vec<SocketAddr>,
    account: AccountName,
    password: String,
    register: bool,
    name: Option<MachineName>,
) -> Result<Reply, Failure> {
    fastctx_net::login::open_account(&hub, &account, &password).await.map_err(Failure::new)?;
    login(node, hub, account, password, register, name).await
}

async fn own_key(node: &Node) -> Result<SecretKey, Failure> {
    let world = node.world.clone();
    tokio::task::spawn_blocking(move || key(&world))
        .await
        .map_err(|error| Failure::new(format!("making this computer's key: {error}")))?
        .map_err(|error| Failure::new(format!("making this computer's key: {error}")))
}

/// The name this computer logs in under: its own, as it writes it. Another
/// is taken only while it is on no hub, and becomes its own.
fn asked_name(node: &Node, name: Option<MachineName>) -> Result<MachineName, Failure> {
    let here = node.machine();
    match name {
        Some(name) if name != here && node.connected_anywhere() => Err(Failure::new(format!(
            "this computer is {here} on the hubs it is on, and takes the same name on every hub; log in as {here}"
        ))),
        Some(_) if node.connected_anywhere() => Ok(here),
        Some(name) => Ok(name),
        None => Ok(here),
    }
}

/// Records the membership `joined` gives, renames this computer to the
/// name it got when it was on no hub, and starts or refreshes the link.
async fn logged_in(node: &Arc<Node>, joined: Joined) -> Result<Reply, Failure> {
    let Joined { hub, hub_name, name, account, addresses, cert, relays } = joined;
    // A computer on no hub yet takes the name as the hub writes it; one on
    // other hubs keeps its own spelling of the same name.
    if name.as_str() != node.machine().as_str() && !(name == node.machine() && node.connected_anywhere()) {
        if node.connected_anywhere() {
            return Err(Failure::new(format!(
                "the hub {hub_name} knows this computer as {name}, and the other hubs it is on as {}; it logged in, but cannot be both",
                node.machine()
            )));
        }
        let names = crate::machine::rename(&node.world, &name)
            .map_err(|error| Failure::new(format!("renaming this computer to {name}: {error}")))?;
        node.set_names(names);
    }
    match node.link_of(&hub) {
        // Logged in again where the hub is reached now, as after a move this
        // computer did not follow: the link connects there from now on.
        Some(link) if !addresses.is_empty() && link.hub_addresses() != addresses => {
            link.reach_hub_at(addresses);
            if let Err(error) = link.refresh().await {
                node.log.line(format_args!("logged in again; the link connects to the hub anew: {error}"));
            }
        }
        Some(link) => {
            link.refresh().await.map_err(|error| Failure::new(format!("logged in, but asking the hub: {error}")))?;
        }
        None => {
            let membership = Membership {
                id: hub.clone(),
                hub_name: hub_name.clone(),
                name: name.clone(),
                addresses,
                cert,
                kind: Kind::Computer,
                relays,
            };
            let net = net_dir(&node.world, &hub);
            net.prepare()
                .and_then(|()| fastctx_net::link::state::write(&net.membership(), &membership))
                .map_err(|error| Failure::new(format!("logged in, but recording the membership failed: {error}")))?;
            start(node, membership).map_err(|error| Failure::new(format!("starting the link to the hub: {error}")))?;
            crate::ledger::joined(node, &hub);
            let (carrying, at) = (node.clone(), hub.clone());
            tokio::spawn(async move { crate::own_collab::carry(&carrying, &at).await });
        }
    }
    node.log.line(format_args!("logged in to {account} on {hub_name} as {name}"));
    Ok(Reply::LoggedIn { hub: hub_name, name, account })
}

/// The hub `hub` calls this computer `to` now, no longer `from`
/// (`world-network.md`, 机器的名字). It takes the name, keeping the one it had
/// among those that still name it, and asks its other hubs to call it so
/// too: a computer has one name on every hub it is on.
pub(crate) fn follow_rename(node: &Arc<Node>, hub: &HubId, from: &MachineName, to: &MachineName) {
    if node.machine().as_str() != to.as_str() {
        let names = crate::machine::renamed(&node.machine_names(), to);
        if let Err(error) = crate::machine::keep(&node.world, &names) {
            node.log.line(format_args!("recording that this computer is {to} now: {error}"));
        }
        node.set_names(names);
        node.log.line(format_args!("this computer is {to} now, no longer {from}"));
    }
    // Every session's tools say where its shell is.
    node.tools_changed(None);
    node.ways_changed();
    for link in node.links().into_iter().filter(|link| link.hub() != hub) {
        let node = node.clone();
        tokio::spawn(async move { same_name(&node, &link).await });
    }
}

/// Asks the hub of `link` to call this computer by its name now, when it
/// calls it otherwise, as the account it is registered under there, which
/// may rename its own machine.
async fn same_name(node: &Arc<Node>, link: &Link) {
    let (here, there) = (node.machine(), link.name());
    if here.as_str() == there.as_str() {
        return;
    }
    let view = link.view();
    let hub = view.hub_name.clone();
    let Some(account) = view.me().and_then(|me| me.home.clone()) else {
        node.log.line(format_args!("the hub {hub} calls this computer {there}, and it is no machine there to rename"));
        return;
    };
    if !view.renames {
        node.log.line(format_args!(
            "the hub {hub} calls this computer {there}, not {here}, and renames no machine until it is updated"
        ));
        return;
    }
    match link.manage(&account, Change::Rename { machine: there.clone(), to: here.clone() }).await {
        Ok(_) => node.log.line(format_args!("the hub {hub} calls this computer {here} now too")),
        Err(error) => node
            .log
            .line(format_args!("the hub {hub} calls this computer {there}, and did not rename it {here}: {error}")),
    }
}

/// The link of the hub named `hub`, or of the one hub when none is named.
pub fn hub_link(node: &Node, hub: Option<&HubName>) -> Result<Arc<Link>, Failure> {
    let links = node.links();
    match hub {
        Some(hub) => links
            .into_iter()
            .find(|link| link.hub_name() == *hub)
            .ok_or_else(|| Failure::new(format!("this computer is not logged in to a hub named {hub}"))),
        None => match links.len() {
            0 => Err(Failure::new("this computer is logged in to no hub")),
            1 => Ok(links.into_iter().next().expect("one")),
            _ => Err(Failure::new(format!(
                "this computer is on more than one hub; name one of {}",
                links.iter().map(|link| link.hub_name().to_string()).collect::<Vec<_>>().join(", ")
            ))),
        },
    }
}

/// Makes `change` as `account` on the hub named `hub`. Returns a token when
/// it made one.
pub async fn change(
    node: &Arc<Node>,
    hub: Option<&HubName>,
    account: &AccountName,
    change: AccountChange,
) -> Result<Option<String>, Failure> {
    let link = hub_link(node, hub)?;
    match change {
        AccountChange::Logout => log_out(node, &link, account).await.map(|()| None),
        AccountChange::Seen { name } => crate::admitted::shown(node, &link.membership().id, account, &name)
            .map(|()| None)
            .map_err(|error| Failure::new(format!("recording what was seen: {error}"))),
        AccountChange::Rename { machine, to } if link.view().is_me(&machine) => {
            let from = link.name();
            link.account_change(account, AccountChange::Rename { machine, to: to.clone() })
                .await
                .map_err(Failure::new)?;
            // The hub took it: this computer is `to` from now on, before its
            // next connection is welcomed under that name.
            follow_rename(node, link.hub(), &from, &to);
            Ok(None)
        }
        change => link.account_change(account, change).await.map_err(Failure::new),
    }
}

/// Renames this computer `to` (`world-network.md`, 机器的名字). On no hub it
/// is its own to name. On hubs, the first where it is a registered machine
/// renames it, and the others follow: one asks, so that a refusal there, a
/// name taken, is the answer.
pub async fn rename_here(node: &Arc<Node>, to: MachineName) -> Result<(), Failure> {
    let from = node.machine();
    if from.as_str() == to.as_str() {
        return Ok(());
    }
    let links = node.links();
    if links.is_empty() {
        let names = crate::machine::renamed(&node.machine_names(), &to);
        crate::machine::keep(&node.world, &names)
            .map_err(|error| Failure::new(format!("renaming this computer to {to}: {error}")))?;
        node.set_names(names);
        node.log.line(format_args!("this computer is {to} now, no longer {from}"));
        node.tools_changed(None);
        return Ok(());
    }
    let Some((link, account)) = links.iter().find_map(|link| {
        let view = link.view();
        view.me().and_then(|me| me.home.clone()).map(|account| (link.clone(), account))
    }) else {
        return Err(Failure::new("this computer is a machine of no hub it is on, so no hub renames it"));
    };
    let machine = link.name();
    link.account_change(&account, AccountChange::Rename { machine: machine.clone(), to: to.clone() })
        .await
        .map_err(Failure::new)?;
    follow_rename(node, link.hub(), &machine, &to);
    Ok(())
}

/// Logs this computer out of `account`; with no login left on that hub,
/// its membership is put aside and the link stops. A managed machine leaves
/// only once its owner took it off.
async fn log_out(node: &Arc<Node>, link: &Arc<Link>, account: &AccountName) -> Result<(), Failure> {
    if link.view().me().is_some_and(|me| me.managed) {
        crate::managed::stays(node)?;
    }
    let last = link.view().me().is_none_or(|me| me.logins.iter().all(|login| login == account));
    // Leaving for good is written down before the hub is told: a computer
    // stopped in between finishes leaving when it starts (`load`).
    if last {
        fastctx_platform::publish(
            &hub_dir(&node.world, &link.membership().id).join(LEAVING),
            account.as_str().as_bytes(),
            fastctx_platform::Permissions::Private,
        )
        .map_err(|error| Failure::new(format!("recording that this computer leaves: {error}")))?;
    }
    let answer = link.manage(account, Change::Logout).await;
    if !last {
        return answer.map(|_| ()).map_err(Failure::new);
    }
    // The hub ends the session of a member with no login left, and may do
    // so before its answer is out: being stopped is the answer then. A hub
    // that cannot be reached is not told, and the computer leaves it all
    // the same: there it stays a machine away until it is removed.
    if answer.is_err() && matches!(link.status(), Status::Online { .. }) {
        let _ = std::fs::remove_file(hub_dir(&node.world, &link.membership().id).join(LEAVING));
        answer.map_err(Failure::new)?;
    }
    let aside = put_aside(node, link).await?;
    node.log.line(format_args!("logged out of the hub {}; its records are in {}", link.hub_name(), aside.display()));
    Ok(())
}

/// Leaves a network whose administrator dissolved it: its membership is put
/// aside as a logout puts it, and the user is told once. The notice is kept
/// first, so no interface sees the network gone without it.
async fn left_dissolved(node: &Arc<Node>, hub: &fastctx_types::access::HubId) -> Result<(), Failure> {
    static NOTICES: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let Some(link) = node.link_of(hub) else { return Ok(()) };
    let name = link.hub_name();
    {
        let _one = NOTICES.lock().expect("never poisoned");
        let path = node.world.dissolved();
        let mut seen: Vec<fastctx_types::accounts::Dissolution> =
            fastctx_net::link::state::read_or(&path, Vec::new()).map_err(|error| Failure::new(error.to_string()))?;
        seen.retain(|said| said.hub != name);
        seen.push(fastctx_types::accounts::Dissolution { hub: name.clone(), at: Timestamp::now() });
        fastctx_net::link::state::write(&path, &seen).map_err(|error| Failure::new(error.to_string()))?;
    }
    let aside = put_aside(node, &link).await?;
    node.log.line(format_args!("the network {name} was dissolved; its records are in {}", aside.display()));
    Ok(())
}

/// The networks said dissolved were seen: no interface says so again.
pub fn seen_dissolved(node: &Node) -> Result<(), Failure> {
    match std::fs::remove_file(node.world.dissolved()) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(Failure::new(error.to_string())),
        _ => {}
    }
    node.shown_changed();
    Ok(())
}

/// Stops the link of a network this computer leaves and puts its records
/// aside; where they are put.
async fn put_aside(node: &Arc<Node>, link: &Arc<Link>) -> Result<std::path::PathBuf, Failure> {
    let hub = link.membership().id.clone();
    // Leaving its last network, the computer keeps the model collaboration
    // that named it as its own (`own_collab`).
    if node.links().iter().all(|other| other.membership().id == hub) {
        crate::own_collab::take_back(node, link);
    }
    link.stop();
    crate::mcp::store::leave(node, &hub).map_err(Failure::new)?;
    node.links.write().expect("never poisoned").remove(&hub);
    node.ways_changed();
    // Its view is gone, and every interface shows the World without it.
    node.views_changed();
    // The machines each session may use, and who may ask for colleagues,
    // are in its tools.
    node.tools_changed(None);
    // What the firewall let in for direct connections on that hub goes with
    // the membership.
    let closing = hub.clone();
    match tokio::task::spawn_blocking(move || crate::firewall::open(&closing, &Default::default())).await {
        Ok(Ok(_)) => {}
        Ok(Err(why)) => node.log.line(format_args!("closing the firewall for direct connections: {why}")),
        Err(error) => node.log.line(format_args!("closing the firewall for direct connections: {error}")),
    }
    let net = net_dir(&node.world, &hub);
    let aside = node.world.path().join(format!("net-left-{}-{}", hub, Timestamp::now().strftime("%Y%m%dT%H%M%SZ")));
    std::fs::rename(net.path(), &aside)
        .map_err(|error| Failure::new(format!("putting {} aside: {error}", net.path().display())))?;
    Ok(aside)
}

/// Every hub this computer is on, as its views show them.
pub fn accounts(node: &Node) -> AccountsReply {
    let dissolved = fastctx_net::link::state::read_or(&node.world.dissolved(), Vec::new()).unwrap_or_default();
    let hubs = node
        .links()
        .iter()
        .map(|link| {
            let mut accounts = link.accounts();
            accounts.arrivals = crate::admitted::fresh(node, &accounts.hub);
            accounts
        })
        .collect();
    let names = node.machine_names();
    AccountsReply { machine: names.name, was: names.was, hubs, dissolved }
}

/// What this machine is like.
pub fn facts(node: &Node) -> MachineFacts {
    let known = |path: std::io::Result<std::path::PathBuf>| {
        path.map(|path| native(&path)).unwrap_or_else(|error| format!("(not known: {error})"))
    };
    MachineFacts {
        turn_debts: true,
        goal_runtime: node.goals.available(),
        goal_colleagues: true,
        goal_waits: true,
        goal_questions: true,
        goal_question_words: true,
        goal_commands: true,
        goal_reports: true,
        name: node.machine(),
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        home: known(fastctx_platform::user_home()),
        temp: known(fastctx_platform::temp_dir()),
        path_separator: if cfg!(windows) { ';' } else { ':' },
        case_sensitive: !cfg!(any(windows, target_os = "macos")),
        version: env!("CARGO_PKG_VERSION").to_string(),
        sentinels: true,
        sentinel_look: true,
        completion_receipts: true,
        control_letters: true,
        run_stops: true,
        upload_resumes: true,
        attachment_references: true,
        stats: true,
        mcp: true,
        mcp_import: true,
        page_accounts: true,
        attachment_places: true,
        attachment_runs: true,
        memory_folders: true,
        collaboration_tree: true,
        shell: shell(),
        form: Some(form()),
        node_lifetime: Some(Box::new(node.lifetime.clone())),
    }
}

/// What kind of device this is, asked once: it does not change while the
/// node runs.
fn form() -> fastctx_types::world::Form {
    static FORM: std::sync::OnceLock<fastctx_types::world::Form> = std::sync::OnceLock::new();
    *FORM.get_or_init(fastctx_platform::form::form)
}

/// The bash `run` starts here and its version. Once found it is kept for the
/// node's life; until then it is looked for each time it is asked, since a
/// managed computer often has Git installed after the node that keeps
/// running.
fn shell() -> String {
    static SHELL: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    if let Some(shell) = SHELL.get() {
        return shell.clone();
    }
    let bash = match fastctx_hands::run::shell::bash() {
        Ok(bash) => bash,
        Err(error) => return format!("no bash: {error}"),
    };
    let mut asking = std::process::Command::new(&bash);
    asking.arg("--version").stdin(std::process::Stdio::null());
    let version = fastctx_platform::bounded::output(asking, "bash --version", std::time::Duration::from_secs(10))
        .ok()
        .and_then(|output| {
            // "GNU bash, version 5.2.37(1)-release (x86_64-pc-msys)"
            let text = String::from_utf8_lossy(&output.stdout).into_owned();
            let rest = text.lines().next()?.split("version ").nth(1)?.to_string();
            Some(rest.split(['(', ' ']).next()?.to_string())
        });
    let version = version.map(|version| format!("bash {version}")).unwrap_or_else(|| "bash".to_string());
    let shell = if cfg!(windows) { format!("{version} (Git for Windows, MSYS)") } else { version };
    SHELL.get_or_init(|| shell).clone()
}

/// A path as the World writes it: in full, with no trailing slash, and on
/// Windows with forward slashes. Windows hands out some paths, such as the
/// temporary directory, in their short 8.3 form, which resolving undoes.
pub(crate) fn native(path: &std::path::Path) -> String {
    let resolved = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let mut text = resolved.to_string_lossy().into_owned();
    if cfg!(windows) {
        if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
            text = format!(r"\\{rest}");
        } else if let Some(rest) = text.strip_prefix(r"\\?\") {
            text = rest.to_string();
        }
        text = text.replace('\\', "/");
    }
    while text.len() > 1 && text.ends_with('/') && !text.ends_with(":/") {
        text.pop();
    }
    text
}

/// What one hub's link hands this node.
struct Handler {
    node: Weak<Node>,
    hub: HubId,
}

impl fastctx_net::link::Node for Handler {
    fn login_homes(&self, agent: &fastctx_types::AgentName) -> Result<Vec<(std::path::PathBuf, bool)>, String> {
        let node = self.node.upgrade().ok_or("the node is stopping")?;
        crate::logins::identities(&node, agent).map_err(|error| error.message)
    }
    fn acquire(
        &self,
        request: fastctx_net::link::admission::Request,
    ) -> BoxFuture<Result<fastctx_net::link::admission::Admitted, Failure>> {
        let node = self.node.upgrade();
        let hub = self.hub.clone();
        Box::pin(async move {
            let node = node.ok_or_else(|| Failure::not_run("the node is stopping"))?;
            crate::intake::take(&node, hub, request).await
        })
    }

    fn answering(&self, call: &RemoteCall) -> Result<Option<Box<dyn Send>>, Failure> {
        // A stream, such as a watch or an `on`, goes on for as long as it
        // goes on: a node handing the World on does not wait for it. A copy
        // asked of this machine holds it back only while it moves bytes
        // (`copy`).
        if !call.contract().held {
            return Ok(None);
        }
        let node = self.node.upgrade().ok_or_else(|| Failure::not_run("the node is stopping"))?;
        node.handoff.call().map(|guard| Some(Box::new(guard) as Box<dyn Send>)).ok_or_else(|| {
            Failure::not_run(format!(
                "the node on {} is handing over to its successor; ask again in a moment",
                node.machine()
            ))
        })
    }

    fn call(
        &self,
        caller: Caller,
        call: RemoteCall,
        stream: Option<fastctx_net::data::Stream>,
    ) -> BoxFuture<RemoteResult> {
        let node = self.node.upgrade();
        let hub = self.hub.clone();
        let doing = format!("{} from {}", crate::labels::call_of(&call), caller.machine);
        Box::pin(crate::gate_waits::asked(Box::pin(async move {
            let _doing = fastctx_platform::doing::doing(doing);
            // What turns a call away before it is admitted did nothing.
            let Some(node) = node else {
                return Err(Failure::not_run("the node is stopping"));
            };
            let link = node.link_of(&hub).ok_or_else(|| Failure::not_run("the link that brought the call is gone"))?;
            if let Some(town) = crate::admission::town_of(&call)
                && link.view().project_named(&caller.account, town).is_none()
            {
                link.refresh().await.map_err(Failure::not_run)?;
            }
            let asker = match crate::admission::admit(&node, &link, &caller, &call) {
                Ok(asker) => asker,
                Err(refused) => {
                    // Its data connection is closed, not dropped: the other
                    // end then waits for this answer, which says why.
                    if let Some(stream) = stream {
                        stream.close().await;
                    }
                    return Err(refused);
                }
            };
            let from = caller.machine.clone();
            let acting = Acting::one(hub.clone(), caller.account.clone());
            let machine = node.machine();
            if matches!(
                &call,
                RemoteCall::Thumbnail { .. }
                    | RemoteCall::AttachmentThumbnail { .. }
                    | RemoteCall::Lines {
                        request: fastctx_types::lines::Request::Find { .. }
                            | fastctx_types::lines::Request::Changed { .. }
                            | fastctx_types::lines::Request::Close { .. }
                    }
            ) {
                let mut stream =
                    stream.ok_or_else(|| Failure::new("a cancellable read requires its data connection"))?;
                let reading = async {
                    match call {
                        RemoteCall::Thumbnail { request } => {
                            crate::thumbnail::call(&node, None, request).await.map(RemoteReply::Thumbnail)
                        }
                        RemoteCall::AttachmentThumbnail { attachment, max } => {
                            crate::thumbnail::attachment(&node, attachment, max).await.map(RemoteReply::Thumbnail)
                        }
                        RemoteCall::Lines { request } => {
                            crate::lines::call(&node, from, None, request).await.map(RemoteReply::Lines)
                        }
                        _ => unreachable!("cancellable reads"),
                    }
                };
                return tokio::select! { result = reading => result, () = stream.closed() => Err(Failure::new("the read's caller disconnected")) };
            }
            match call {
                RemoteCall::Mcp { mcp } => crate::mcp::answer(&node, &hub, &caller.account, mcp)
                    .await
                    .map(|result| RemoteReply::Mcp { result }),
                RemoteCall::McpServe { name } => {
                    let stream = stream.ok_or_else(|| Failure::new("MCP requires a data connection"))?;
                    crate::mcp::serve(&node, &hub, &caller.account, &name, stream).await.map(|()| RemoteReply::Set)
                }
                RemoteCall::PageAction { source, action } => {
                    crate::pages::actions::answer(&node, &asker, source, action)
                        .await
                        .map(|result| RemoteReply::PageAction { result })
                }
                RemoteCall::Relay { made } => crate::devices::relay(&node, &asker, made).await,
                // A resident of the calling machine works here (`world-run.md`,
                // 这里).
                RemoteCall::Run { resident, town, request, answer_receipts, dispatch } => {
                    let account = HubAccount { hub: hub.clone(), account: caller.account.clone() };
                    crate::here::serve(&node, from, account, resident, town, request, answer_receipts, dispatch).await
                }
                RemoteCall::WordsCame { resident } => Ok(crate::here::words_came(&node, from, resident)),
                RemoteCall::TurnInterrupted { resident, dispatch } => match dispatch {
                    Some(dispatch) => crate::run_stops::serve(
                        &node,
                        &Address { machine: from, resident },
                        &dispatch,
                        &HubAccount { hub: hub.clone(), account: caller.account.clone() },
                    )
                    .await
                    .map(|()| RemoteReply::Set),
                    None => Ok(crate::here::turn_interrupted(&node, from, resident)),
                },
                RemoteCall::Forget { resident, town, job, delivery, mut origin, through_exit } => {
                    if let Some(delivery) = delivery {
                        if job != delivery.job {
                            return Err(Failure::new("the end receipt names another job"));
                        }
                        let owner = match origin.as_mut() {
                            Some(origin) => {
                                origin.account = Some(HubAccount { hub: hub.clone(), account: caller.account.clone() });
                                origin.resident.clone()
                            }
                            None => Address { machine: from, resident },
                        };
                        let account = HubAccount { hub: hub.clone(), account: caller.account.clone() };
                        crate::retention::received_from(
                            &node,
                            &owner,
                            &delivery,
                            origin.as_ref(),
                            through_exit,
                            Some(&account),
                        )
                        .map_err(|error| Failure::new(format!("recording job delivery: {error}")))?;
                        Ok(RemoteReply::Set)
                    } else {
                        Ok(crate::here::forget_here(&node, from, resident, town, job).await)
                    }
                }
                // What an older fastctx sends: files go to another machine only
                // as `on` now (`world-run.md`).
                RemoteCall::Write { .. } => Err(Failure::new(format!(
                    "{machine} takes files from other machines only through `on`; the fastctx on {from} is older and needs updating"
                ))),
                RemoteCall::On { mut origin, sizes, writes_off, piped, source, attachments } => {
                    let stream = stream.ok_or_else(|| Failure::new("an on comes with a data connection"))?;
                    origin.follower = Some(from.clone());
                    origin.account = Some(HubAccount { hub: hub.clone(), account: caller.account.clone() });
                    let how = crate::on::Sent { writes_off, piped, source, attachments };
                    crate::on::serve_on(&node, origin, sizes, how, stream).await
                }
                RemoteCall::PipeOut { mut origin, job } => {
                    let stream = stream.ok_or_else(|| Failure::new("an on's output goes over a data connection"))?;
                    // The account is the one this machine was called for.
                    origin.account = Some(HubAccount { hub: hub.clone(), account: caller.account.clone() });
                    crate::on::serve_pipe(&node, origin, job, stream).await
                }
                RemoteCall::OnReceipt { owner, machine, job, request } => {
                    if owner.machine != from {
                        return Err(Failure::new("only the completion's resident machine asks for its receipt"));
                    }
                    crate::on_delivery::status(&node, &owner, &machine, &job, request.as_ref())
                        .map(|(taken, following)| RemoteReply::OnReceipt { taken, following })
                        .map_err(|error| Failure::new(format!("reading the on receipt: {error}")))
                }
                RemoteCall::Follow { mut origin, job, out_from, err_from } => {
                    if origin.sentinel.is_some() {
                        origin.account = Some(HubAccount { hub: hub.clone(), account: caller.account.clone() });
                    }
                    let stream = stream.ok_or_else(|| Failure::new("following an on needs a data connection"))?;
                    crate::on::serve_follow(&node, origin, job, out_from, err_from, stream).await
                }
                RemoteCall::StopOn { mut origin, job } => {
                    if origin.sentinel.is_some() {
                        origin.account = Some(HubAccount { hub: hub.clone(), account: caller.account.clone() });
                    }
                    crate::on_stop::serve(&node, origin, job).await
                }
                RemoteCall::StopJob { job, by } => {
                    // The user is the user on whichever of their machines they
                    // asked; an agent is named with where it lives.
                    let by = if by == fastctx_types::residents::STOPPED_BY_USER {
                        by
                    } else {
                        format!("{by} on {from}, for {}", caller.account)
                    };
                    crate::stop::here_(&node, &job, &by)
                        .await
                        .map(|record| RemoteReply::JobStopped { record: record.map(Box::new) })
                }
                RemoteCall::Ask { asker: resident, requester_turn, town, label, prompt, started } => {
                    let acting = Acting::one(hub.clone(), caller.account.clone());
                    let asker = Address { machine: from, resident };
                    let colleague = crate::colleagues::ask_here(
                        &node,
                        crate::colleagues::Requester { address: asker, turn: requester_turn },
                        acting,
                        town,
                        label,
                        prompt,
                        started,
                    )
                    .await?;
                    Ok(RemoteReply::Asked { colleague })
                }
                RemoteCall::Talk { to, text, attachments, files_on, id } => {
                    crate::talk::talk_here(&node, &from, &to, text, attachments, files_on.as_ref(), id.as_deref())
                        .await
                        .map(|landed| RemoteReply::Said { landed })
                }
                RemoteCall::Watch { resident, watch, composer, documents, held, now, goal, multiplex } => {
                    let stream = stream
                        .ok_or_else(|| Failure::new("a watch comes with a data connection, and this one had none"))?;
                    if multiplex {
                        crate::talk::multiplex::serve(node.clone(), link.clone(), caller.clone(),
                            fastctx_types::watching::Subscription { resident, composer, documents, held, now, goal }, watch, stream).await
                    } else {
                        crate::talk::serve_watch(
                            &node, &asker, &link, &from, resident, watch, composer, held, now, goal, stream,
                        ).await
                    }
                }
                RemoteCall::Agents => crate::agents::here(&node)
                    .await
                    .map(|reply| RemoteReply::Agents(crate::admission::seen::agents(&link.view(), &asker, reply))),
                RemoteCall::ClaudeConsent { accept, home } => {
                    crate::consent::here(&node, accept, home, format!("the account {} from {from}", caller.account))
                        .await
                        .map(|(accepted, changed)| RemoteReply::ClaudeConsent { accepted, changed })
                }
                RemoteCall::Homes { change } => crate::homes::change(&node, change).await.map(|()| RemoteReply::Set),
                call @ (RemoteCall::AgentLogins
                | RemoteCall::ScanAgents { .. }
                | RemoteCall::AddAgentLogin { .. }
                | RemoteCall::AgentFolder { .. }
                | RemoteCall::SetAgentTitle { .. }
                | RemoteCall::PreviewAgentChange { .. }
                | RemoteCall::ChangeAgentLogin { .. }
                | RemoteCall::ReadAgentLimits { .. }
                | RemoteCall::AgentUsage { .. }
                | RemoteCall::AgentCalls { .. }) => {
                    crate::logins::remote::answer(&node, &link.view(), &asker, call).await
                }
                RemoteCall::Folders { path } => {
                    crate::focus::folders_here(node.machine(), path).await.map(RemoteReply::Folders)
                }
                RemoteCall::Outline { folders, within_ms } => {
                    let within = std::time::Duration::from_millis(within_ms);
                    Ok(RemoteReply::Outlined { outlines: crate::focus::outline_here(folders, within).await })
                }
                RemoteCall::FindFiles { request } => {
                    if let fastctx_types::file_search::FileSearchRequest::Open { scope } = request {
                        let stream =
                            stream.ok_or_else(|| Failure::new("a file search requires its owner's data connection"))?;
                        crate::file_search::serve(&node, &link, from, scope, stream).await
                    } else {
                        crate::file_search::here(&node, from, request).await.map(RemoteReply::FilesFound)
                    }
                }
                RemoteCall::ReadFile { request, read } => {
                    let stream = stream.ok_or_else(|| Failure::new("a file read requires a data connection"))?;
                    crate::file_read::serve(&node, &link, request, read, stream).await
                }
                RemoteCall::ReadAttachment { attachment, read, range } => {
                    let stream = stream.ok_or_else(|| Failure::new("an attachment read requires a data connection"))?;
                    crate::file_read::serve_attachment(&node, &link, attachment, range, read, stream).await
                }
                RemoteCall::Thumbnail { request } => {
                    crate::thumbnail::call(&node, None, request).await.map(RemoteReply::Thumbnail)
                }
                RemoteCall::AttachmentThumbnail { attachment, max } => {
                    crate::thumbnail::attachment(&node, attachment, max).await.map(RemoteReply::Thumbnail)
                }
                RemoteCall::Lines { request } => {
                    if let fastctx_types::lines::Request::Open { path, decoding } = request {
                        let stream =
                            stream.ok_or_else(|| Failure::new("a line source requires its owning data connection"))?;
                        crate::lines::serve(&node, &link, from, path, decoding, stream).await
                    } else {
                        crate::lines::call(&node, from, None, request).await.map(RemoteReply::Lines)
                    }
                }
                RemoteCall::ReadLines { request, read } => {
                    let stream = stream.ok_or_else(|| Failure::new("logical rows require a data connection"))?;
                    crate::lines::reading::remote(&node, &link, from, request, false, read, stream).await
                }
                RemoteCall::ReadLineNotes { request, read } => {
                    let stream = stream.ok_or_else(|| Failure::new("logical rows require a data connection"))?;
                    crate::lines::reading::remote(&node, &link, from, request, true, read, stream).await
                }
                RemoteCall::Open {
                    agent,
                    town,
                    text,
                    model,
                    reasoning_effort,
                    attachments,
                    focus,
                    home,
                    files_on,
                    mcp_off,
                } => {
                    let options =
                        fastctx_types::agents::SessionOptions { model, reasoning_effort, ..Default::default() };
                    let choices = fastctx_types::agents::Choices { account: None, focus, home, files_on, mcp_off };
                    let (resident, landed) =
                        crate::talk::open_here(&node, &from, acting, agent, town, text, options, attachments, choices)
                            .await?;
                    Ok(RemoteReply::Opened { resident, landed })
                }
                RemoteCall::Stop { resident } => {
                    crate::talk::stop_here(&node, &resident).await.map(|stopped| RemoteReply::Stopped { stopped })
                }
                RemoteCall::Manage { resident, action } => {
                    crate::management::here(&node, &resident, action).await.map(RemoteReply::Managed)
                }
                RemoteCall::Control { resident, control, reply_envelope } => {
                    if !reply_envelope
                        && matches!(&control, fastctx_types::session::control::Control::Command { name, .. } if name == "btw")
                    {
                        Err(Failure::not_run(
                            "This client's control reply format is too old for /btw; update the requesting Fastctx before running it.",
                        ))
                    } else {
                        crate::control::control_here(&node, &resident, control).await.map(|controlled| {
                            if reply_envelope {
                                RemoteReply::ControlResult { controlled }
                            } else {
                                RemoteReply::Controlled(controlled)
                            }
                        })
                    }
                }
                RemoteCall::Composer { resident, request } => crate::composer::here(&node, &from, &resident, request)
                    .await
                    .map(|updated| RemoteReply::Composer(Box::new(updated))),
                RemoteCall::ItemDetail { resident, turn, item } => {
                    crate::held_history::item_detail_here(&node, &resident, turn.as_deref(), &item)
                        .await
                        .map(|detail| RemoteReply::ItemDetail { detail })
                }
                RemoteCall::AgentActivity { resident, agent } => crate::agent_activity::here(&node, &resident, &agent)
                    .await
                    .map(|activity| RemoteReply::AgentActivity { activity: Box::new(activity) }),
                RemoteCall::Offers { resident } => crate::control::offers_here(&node, &resident)
                    .await
                    .map(|offers| RemoteReply::Offers(Box::new(crate::admission::seen::offers(&asker, offers)))),
                RemoteCall::WaitingJobs { owner } => {
                    crate::turn_evidence::waiting::jobs_here(&node, &owner, Some(&asker))
                        .map(|jobs| RemoteReply::WaitingJobs { jobs })
                        .map_err(|error| Failure::new(error.to_string()))
                }
                RemoteCall::ConversationDebt { resident, requester } => {
                    crate::turn_evidence::debts::here(&node, &acting, &resident, &requester)
                        .await
                        .map(|debt| RemoteReply::ConversationDebt { debt })
                }
                RemoteCall::Look { id } => {
                    let (looked, mut jobs) = crate::look::look_here(&node, &id).await?;
                    jobs.retain(|job| crate::admission::seen::job(&node, &asker, job));
                    Ok(RemoteReply::Looked {
                        looked: crate::admission::seen::looked(&node, &asker, looked),
                        jobs,
                        read_at: Some(jiff::Timestamp::now()),
                    })
                }
                RemoteCall::Find { resident } => {
                    let lives_here = crate::admission::seen::resident(
                        &node,
                        crate::admission::seen::Looker::Asker(&asker),
                        &resident,
                    );
                    Ok(RemoteReply::Found { lives_here })
                }
                RemoteCall::Who { all, town, since } => crate::server::who_here(&node, all, town, since)
                    .await
                    .map(|reply| RemoteReply::Who(crate::admission::seen::who(&node, &asker, reply))),
                RemoteCall::Since { since, town } => crate::since::here(&node, since, town).await.map(|reply| {
                    RemoteReply::Since(crate::admission::seen::since(
                        &node,
                        crate::admission::seen::Looker::Asker(&asker),
                        reply,
                    ))
                }),
                RemoteCall::Canon { .. } => Err(Failure::not_run(fastctx_types::memory::LEGACY)),
                RemoteCall::MemoryFolders { town } => {
                    crate::memory::folders::here_for(&node, &asker, &town).map(RemoteReply::MemoryFolders)
                }
                RemoteCall::Plan { resident, operation } => {
                    let account =
                        fastctx_types::access::HubAccount { hub: hub.clone(), account: caller.account.clone() };
                    crate::plan::here(&node, &resident, None, None, Some(&account), operation)
                        .await
                        .map(|result| RemoteReply::Plan { result })
                }
                RemoteCall::EnterProject { resident, town } => {
                    let account =
                        fastctx_types::access::HubAccount { hub: hub.clone(), account: caller.account.clone() };
                    crate::town::enter::here(&node, &resident, &town, Some(&account))
                        .await
                        .map(|map| RemoteReply::Entered { town, map: Box::new(map) })
                }
                RemoteCall::GoalCommand { actor, turn, request } => {
                    let account =
                        fastctx_types::access::HubAccount { hub: hub.clone(), account: caller.account.clone() };
                    crate::goals::commands::here(&node, actor, account, turn, request)
                        .await
                        .map(|updated| RemoteReply::Goal { updated: Box::new(updated) })
                }
                RemoteCall::Goal { resident, request } => {
                    let account =
                        fastctx_types::access::HubAccount { hub: hub.clone(), account: caller.account.clone() };
                    crate::goals::calls::user_here(&node, resident, account, from, request)
                        .await
                        .map(|updated| RemoteReply::Goal { updated: Box::new(updated) })
                }
                RemoteCall::Scratch { owner } => Ok(RemoteReply::Scratch { path: crate::scratch::here(&node, &owner) }),
                RemoteCall::SentinelControl { request } => {
                    if matches!(request, fastctx_types::sentinel_control::Request::Create { .. }) {
                        return Err(Failure::refused("a conversation registers a sentinel through its own command"));
                    }
                    crate::sentinels::control(
                        &node,
                        &crate::town::caller::Caller::User,
                        &link,
                        &caller.account,
                        request,
                    )
                    .await
                    .map(|reply| RemoteReply::SentinelControl(Box::new(reply)))
                }
                RemoteCall::SentinelHome { request } => {
                    let scope = fastctx_types::access::HubAccount { hub: hub.clone(), account: caller.account.clone() };
                    crate::sentinels::home_call(&node, &scope, &request)
                        .await
                        .map(|reply| RemoteReply::SentinelHome(Box::new(reply)))
                        .map_err(Failure::new)
                }
                RemoteCall::SentinelSettings { change } => {
                    crate::sentinels::settings(&node, &link, &caller.account, change)
                        .await
                        .map(|reply| RemoteReply::SentinelSettings(Box::new(reply)))
                }
                RemoteCall::SentinelKey { key, value } => {
                    let scope = fastctx_types::access::HubAccount { hub: hub.clone(), account: caller.account.clone() };
                    crate::sentinels::keys::keep_here(&node, &scope, key, value).await.map(RemoteReply::SentinelKeys)
                }
                RemoteCall::SentinelKeys => {
                    let scope = fastctx_types::access::HubAccount { hub: hub.clone(), account: caller.account.clone() };
                    crate::sentinels::keys::status_here(&node, &scope).map(RemoteReply::SentinelKeys)
                }
                RemoteCall::Facts => Ok(RemoteReply::Facts(facts(&node))),
                RemoteCall::Collaboration { resident, colleagues } => {
                    let acting = Acting::one(asker.hub.clone(), asker.account.clone());
                    crate::colleagues::tree::here(&node, &resident, &acting, colleagues)
                        .await
                        .map(|tree| RemoteReply::Collaboration(Box::new(tree)))
                }
                RemoteCall::AttachmentReferences { conversation, attachments } => {
                    if !node.is_me(&conversation.machine)
                        || attachments.iter().any(|file| file.conversation().resident != conversation.resident)
                    {
                        return Err(Failure::not_run("the attachments name another conversation home"));
                    }
                    tokio::task::spawn_blocking(move || crate::attachments::lifetime::referenced(&node, &attachments))
                        .await
                        .map_err(|error| Failure::new(error.to_string()))?
                        .map(|used| RemoteReply::AttachmentReferences { used })
                }
                RemoteCall::UpdateWorld | RemoteCall::Updates { .. } => {
                    let update = match call {
                        RemoteCall::Updates { update } => Some(update),
                        _ => None,
                    };
                    let asker =
                        crate::updates::Asker { by: caller.account.clone(), hub: hub.clone(), scheduled_at: None };
                    crate::updates::remote(&node, asker, update)
                        .await
                        .map(|state| RemoteReply::Update(Box::new(state)))
                        .map_err(Failure::new)
                }
                RemoteCall::Book { query } => crate::activity_book::answer_here(&node, Some(&acting), query)
                    .await
                    .map(|answer| RemoteReply::Book(Box::new(answer))),
                RemoteCall::Stats { after, seen } => {
                    crate::stats::here(&node, after, seen).await.map(|stats| RemoteReply::Stats(Box::new(stats)))
                }
                RemoteCall::MemoryRoom { bytes } => {
                    crate::memory_room::answer(&node, bytes).map(|bytes| RemoteReply::MemoryRoom { bytes })
                }
                RemoteCall::Interfaces => {
                    crate::hub_arrival::interfaces().map(|addresses| RemoteReply::Interfaces { addresses })
                }
                RemoteCall::CopySend { from: path, to } => {
                    let accepted = crate::copy::accept(stream).await?;
                    let from = fastctx_types::copy::Place { machine: None, hub: None, path };
                    let result = Box::pin(crate::copy::copy(&node, &acting, from, to)).await.map(RemoteReply::Copied);
                    drop(accepted);
                    result
                }
                RemoteCall::CopyFetch { from: path } => {
                    let stream = stream
                        .ok_or_else(|| Failure::new("a copy comes with a data connection, and this one had none"))?;
                    crate::copy::give(&node, &path, stream).await.map(RemoteReply::Copied)
                }
                RemoteCall::CopyReceive { to, manifest_bytes, manifest_sha256, resumes } => {
                    let stream = stream
                        .ok_or_else(|| Failure::new("a copy comes with a data connection, and this one had none"))?;
                    crate::copy::receive(&node, to, manifest_bytes, manifest_sha256, resumes, stream)
                        .await
                        .map(RemoteReply::Copied)
                }
                RemoteCall::AttachmentPlaces { target, preferred } => {
                    crate::attachments::destinations::places(&node, &from, &target, &acting, preferred.as_ref())
                        .map(RemoteReply::AttachmentPlaces)
                }
                RemoteCall::ReserveAttachment { target, file, operation, composer } => {
                    crate::attachments::check_project(&node, &acting, &target).await?;
                    crate::attachments::sent::reserve(&node, &from, &from, &acting, target, file, operation, composer)
                        .await
                        .map(RemoteReply::Uploaded)
                }
                RemoteCall::CommitAttachment { attachment, preview } => {
                    crate::attachments::sent::commit(&node, &from, &acting, attachment, preview)
                        .await
                        .map(crate::attachments::remote_reply)
                }
                RemoteCall::Place { resident, attachment } => {
                    let stream = stream
                        .ok_or_else(|| Failure::new("a file comes with a data connection, and this one had none"))?;
                    crate::attachments::take_placed(&node, &from, resident, attachment, stream)
                        .await
                        .map(RemoteReply::Uploaded)
                }
                RemoteCall::Upload { target, file, composer, resumes } => {
                    crate::attachments::check_project(&node, &acting, &target).await?;
                    let stream = stream.ok_or_else(|| Failure::new("an upload requires its data connection"))?;
                    crate::attachments::receive(&node, &from, &acting, target, file, composer, resumes, stream)
                        .await
                        .map(crate::attachments::remote_reply)
                }
            }
        })))
    }

    fn letter(
        &self,
        caller: Caller,
        purpose: Purpose,
        id: LetterId,
        letter: Letter,
    ) -> BoxFuture<io::Result<Result<(), String>>> {
        let from = caller.machine.clone();
        let received = caller.received.clone();
        let node = self.node.upgrade();
        let hub = self.hub.clone();
        Box::pin(async move {
            let Some(node) = node else {
                return Err(io::Error::other("the node is stopping"));
            };
            let _answering = node.handoff.call().ok_or_else(|| io::Error::other("the node is handing over"))?;
            let original = match &letter {
                Letter::Fragment { id, .. } => id.as_str(),
                _ => id.as_str(),
            };
            let identity = crate::letters::Identity::new(&node, caller.key, original, &caller.names);
            let handoff = crate::letters::Handoff::begin(&node, &identity).await;
            let finished = handoff.finished().await?;
            let (letter, completion) = match letter {
                Letter::Fragment { index, total, text, .. } => {
                    match crate::letters::receive(&node, &identity, index, total, &text)? {
                        Ok(Some((letter, completion))) => (letter, Some(completion)),
                        Ok(None) => return Ok(Ok(())),
                        Err(reason) => return Ok(Err(reason)),
                    }
                }
                letter => (letter, None),
            };
            if finished {
                if let Some(completion) = completion {
                    completion.finish()?;
                }
                return Ok(Ok(()));
            }
            if let Err(refused) = crate::admission::letters::admit(&node, &hub, &caller, purpose, &letter) {
                if let Letter::Control { command } = &letter {
                    let failure = crate::control_letters::admit(&node, &hub, &caller, command)
                        .err()
                        .unwrap_or_else(|| Failure::refused(refused.clone()));
                    if !crate::obligations::permanent(&failure) {
                        return Err(io::Error::other(failure.message));
                    }
                    crate::control_letters::keep(&node, &hub, &caller, &identity, *command.clone(), Some(failure))?;
                    handoff.finish(received.as_ref()).await?;
                    if let Some(completion) = completion {
                        completion.finish()?;
                    }
                    return Ok(Ok(()));
                }
                return Ok(Err(refused));
            }
            let delivery_id = match &letter {
                Letter::Speak { to, .. } | Letter::Notice { to, .. } => {
                    identity.delivery(&node.world.conversation(to))?
                }
                _ => identity.id.clone(),
            };
            let result = async {
                match letter {
                    Letter::ControlResult { request, result } => {
                        crate::control_posts::receive(&node, &hub, &caller, &request, result)
                    }
                    Letter::Control { command } => {
                        crate::control_letters::keep(&node, &hub, &caller, &identity, *command, None)?;
                        Ok(Ok(()))
                    }
                    Letter::Fragment { .. } => unreachable!("nested fragments are rejected"),
                    Letter::JobFacts { to, update } => {
                        crate::run_facts::receive(&node, &to, &update)?;
                        Ok(Ok(()))
                    }
                    Letter::Notice { to, notice } => {
                        let notice = *notice;
                        if let Notice::JobEnded { job, machine, scratch, .. } = &notice {
                            if !scratch.is_empty() {
                                let link = node
                                    .link_of(&hub)
                                    .ok_or_else(|| io::Error::other("the sending hub is no longer connected"))?;
                                crate::scratch::observed(&node, &to, &node.written(&link, &from), scratch)?;
                            }
                            crate::here::ended_elsewhere(&node, &to, machine, job);
                        }
                        // A colleague's question, a sentinel's finding and a goal's
                        // report are owed no answer.
                        let owed = if notice.is_question()
                            || matches!(&notice, Notice::Sentinel { .. } | Notice::Words { goal_report: Some(_), .. })
                        {
                            Owed::Nobody
                        } else {
                            Owed::Working
                        };
                        let delivery = Delivery { id: delivery_id, at: Timestamp::now(), notice };
                        if crate::on_delivery::park_from(&node, &to, &delivery, Some(&identity))? {
                            return Ok(Ok(()));
                        }
                        crate::received::keep(&node, &to, delivery, owed)?;
                        Ok(Ok(()))
                    }
                    Letter::Speak { to, from: speaker, text, .. } => {
                        let address = Address { machine: from, resident: speaker };
                        node.residents.set_departed(&node.world, &address, false, Some((hub.as_str(), &identity)))?;
                        let notice = Notice::Words {
                            from: Speaker::Resident { address: address.clone() },
                            text,
                            goal_report: None,
                            goal_question: None,
                        };
                        let delivery = Delivery { id: delivery_id, at: Timestamp::now(), notice };
                        crate::received::keep(&node, &to, delivery, Owed::Speaker(address))?;
                        Ok(Ok(()))
                    }
                    Letter::Left { resident } => {
                        crate::colleagues::departed(
                            &node,
                            Address { machine: from, resident },
                            (hub.as_str(), &identity),
                        )?;
                        Ok(Ok(()))
                    }
                }
            }
            .await;
            if matches!(result, Ok(Ok(()))) {
                handoff.finish(received.as_ref()).await?;
                if let Some(completion) = completion {
                    completion.finish()?;
                }
            }
            result
        })
    }

    fn ledger_changed(&self, town: Option<fastctx_types::access::TownId>) {
        let Some(node) = self.node.upgrade() else { return };
        crate::ledger::syncing::changed(&node, &self.hub, town);
    }

    fn renamed(&self, from: &MachineName, to: &MachineName) {
        if let Some(node) = self.node.upgrade() {
            follow_rename(&node, &self.hub, from, to);
        }
    }

    /// A network dissolved is left here as well: the computer is on it no
    /// more, whoever made it (`world-network.md`, 解散网络).
    fn place_ended(&self, dissolved: bool, then: Option<fastctx_types::uninstall::Then>) {
        let Some(node) = self.node.upgrade() else { return };
        let hub = self.hub.clone();
        if let Err(error) = crate::mcp::store::leave(&node, &hub) {
            node.log.line(format_args!("removing MCP commands after leaving the network: {error}"));
        }
        node.views_changed();
        if dissolved {
            tokio::spawn(async move {
                if let Err(failure) = left_dissolved(&node, &hub).await {
                    node.log.line(format_args!("leaving the dissolved network: {failure}"));
                }
            });
            return;
        }
        let Some(then) = then else { return };
        let Some(membership) = node.link_of(&hub).map(|link| link.dir().to_path_buf()) else { return };
        tokio::spawn(async move { crate::removed::uninstall(&node, &membership, then).await });
    }

    /// Connected again: a hub that calls this computer otherwise than the
    /// others, as one away when another renamed it, is asked to follow.
    fn connected(&self) {
        let Some(node) = self.node.upgrade() else { return };
        let Some(link) = node.link_of(&self.hub) else { return };
        tokio::spawn(async move { same_name(&node, &link).await });
    }

    /// Someone may have logged in to an account this computer is logged in
    /// to: its user is told who, from the views before and after.
    fn account_record_changed(&self, change: &fastctx_types::account_records::Changed) {
        if change.metadata.kind == fastctx_types::plan::KIND
            && let Some(node) = self.node.upgrade()
        {
            crate::context::request_all(&node);
        }
    }

    fn directory_changed(&self) {
        let Some(node) = self.node.upgrade() else { return };
        if let Err(error) = crate::mcp::store::sweep(&node, &self.hub) {
            node.log.line(format_args!("collecting removed MCP commands: {error}"));
        }
        crate::admitted::notice(&node, &self.hub);
        crate::hub_arrival::check(&node, &self.hub);
        // The machines each session may use are in its tools' descriptions.
        node.tools_changed(None);
        node.views_changed();
        let contexts = node.clone();
        tokio::spawn(async move { crate::focus::refresh_all(&contexts).await });
        // A project shared with an account here since is taken to be listed.
        if let Some(link) = node.link_of(&self.hub) {
            tokio::spawn(async move {
                crate::ledger::letgo::let_go(&node, &link).await;
                let mut again = crate::again::Again::default();
                loop {
                    match crate::ledger::sync(&node, link.hub(), None).await {
                        Ok(()) => break,
                        Err(error) => {
                            node.log.line(format_args!("taking the projects of this computer's accounts: {error}"));
                            if !matches!(link.status(), Status::Online { .. }) {
                                break;
                            }
                            tokio::time::sleep(again.next()).await;
                        }
                    }
                }
                // One deleted, or no longer this computer's to keep, goes.
                crate::ledger::letgo::let_go(&node, &link).await;
                node.shown_changed();
            });
        }
    }

    fn ways_changed(&self) {
        if let Some(node) = self.node.upgrade() {
            node.ways_changed();
            node.shown_changed();
        }
    }

    fn afoot_changed(&self) {
        if let Some(node) = self.node.upgrade() {
            node.shown_changed();
        }
    }

    fn versions_changed(&self) {
        if let Some(node) = self.node.upgrade() {
            crate::updates::heard(&node);
        }
    }

    fn retain_package(&self, version: &str) -> Box<dyn Send + Sync> {
        match self.node.upgrade() {
            Some(node) => Box::new(crate::updates::retain_package(&node, version)),
            None => Box::new(()),
        }
    }

    fn relay(
        &self,
        fetch: fastctx_types::update::Fetch,
        progress: tokio::sync::watch::Sender<fastctx_types::update::PackageProgress>,
    ) -> fastctx_net::link::BoxFuture<Result<std::path::PathBuf, String>> {
        let node = self.node.upgrade();
        Box::pin(async move {
            match node {
                Some(node) => crate::updates::relay(&node, fetch, progress).await,
                None => Err("the node is stopping".to_string()),
            }
        })
    }

    fn forget(
        &self,
        forgetting: fastctx_net::wire::Forgetting,
    ) -> fastctx_net::link::BoxFuture<fastctx_net::wire::Forgot> {
        let (node, hub) = (self.node.upgrade(), self.hub.clone());
        Box::pin(async move {
            match node {
                Some(node) => crate::forget::told(&node, &hub, forgetting).await,
                None => fastctx_net::wire::Forgot::Failed { reason: "the node is stopping".to_string() },
            }
        })
    }

    fn open_ports(&self, ports: &std::collections::BTreeSet<u16>) -> Result<fastctx_net::wire::Firewall, String> {
        crate::firewall::open(&self.hub, ports)
    }

    fn log(&self, line: String) {
        if let Some(node) = self.node.upgrade() {
            node.log.line(line);
        }
    }
}

/// The machines a session working for `acting` may work on, each as it is
/// written here: its name, or `name.hub` where another hub this computer is
/// on has a different computer of that name. A computer on several hubs is
/// one machine. One whose agents alone assist it is not among them. With
/// them, those their owners put in groups, group by group, this machine
/// among them where the session works on it: a group of an owner the session
/// does not work for is named with the owner (`bo's lab A`), so two owners'
/// groups of one name stay apart.
pub fn grouped_machines(
    node: &Node,
    acting: &Acting,
    here_too: bool,
) -> (MachineName, Vec<String>, Vec<fastctx_types::world::MachineGroup>) {
    let here = node.machine();
    let mut others: Vec<String> = Vec::new();
    let mut groups: Vec<fastctx_types::world::MachineGroup> = Vec::new();
    let mut keys = Vec::new();
    for link in node.links() {
        let Some(view) = link.last_view() else { continue };
        let accounts = node.accounts_on(&link, acting);
        let mut put = |machine: &fastctx_net::directory::Member, written: &str| {
            if let (Some(group), Some(owner)) = (&machine.group, &machine.home) {
                let name = if accounts.contains(owner) { group.clone() } else { format!("{owner}'s {group}") };
                match groups.iter_mut().find(|known| known.name == name) {
                    Some(known) => known.machines.push(written.to_string()),
                    None => {
                        groups.push(fastctx_types::world::MachineGroup { name, machines: vec![written.to_string()] })
                    }
                }
            }
        };
        for machine in view.machines() {
            if keys.contains(&machine.key) {
                continue;
            }
            // This machine is in its group too, where the session's shell
            // is on it.
            if view.is_me(&machine.name) {
                if here_too {
                    keys.push(machine.key);
                    put(machine, here.as_str());
                }
                continue;
            }
            if view.account_for(&accounts, &view.me, &machine.name, Purpose::Hands).is_some() {
                keys.push(machine.key);
                let written = node.written(&link, &machine.name).to_string();
                put(machine, &written);
                others.push(written);
            }
        }
    }
    others.sort();
    groups.sort_by(|a, b| a.name.cmp(&b.name));
    for group in &mut groups {
        group.machines.sort();
    }
    (here, others, groups)
}
