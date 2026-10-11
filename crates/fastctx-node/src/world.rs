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
    let handler = Arc::new(Handler { node: Arc::downgrade(node), hub: hub.clone() });
    let link = Arc::new(Link::start(net_dir(&node.world, &hub), membership, key, handler)?);
    if let Some(old) = node.links.write().expect("never poisoned").insert(hub, link.clone()) {
        old.stop();
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

/// Logs this computer in with an enrollment token.
pub async fn login_with_token(node: &Arc<Node>, token: &str, name: Option<MachineName>) -> Result<Reply, Failure> {
    let token = fastctx_net::token::Token::parse(token).map_err(Failure::new)?;
    let key = own_key(node).await?;
    let name = asked_name(node, name)?;
    // What the token is for, registering or not, the hub keeps with it.
    let device = Device { name, key: &key, kind: Kind::Computer, register: true };
    let joined = fastctx_net::login::with_token(&token, device).await.map_err(Failure::new)?;
    logged_in(node, joined).await
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

/// The name this computer logs in under: its own. Another is taken only
/// while it is on no hub, and becomes its own.
fn asked_name(node: &Node, name: Option<MachineName>) -> Result<MachineName, Failure> {
    let here = node.machine();
    match name {
        Some(name) if name != here && node.connected_anywhere() => Err(Failure::new(format!(
            "this computer is {here} on the hubs it is on, and takes the same name on every hub; log in as {here}"
        ))),
        Some(name) => Ok(name),
        None => Ok(here),
    }
}

/// Records the membership `joined` gives, renames this computer to the
/// name it got when it was on no hub, and starts or refreshes the link.
async fn logged_in(node: &Arc<Node>, joined: Joined) -> Result<Reply, Failure> {
    let Joined { hub, hub_name, name, account, addresses, cert } = joined;
    if name != node.machine() {
        if node.connected_anywhere() {
            return Err(Failure::new(format!(
                "the hub {hub_name} knows this computer as {name}, and the other hubs it is on as {}; it logged in, but cannot be both",
                node.machine()
            )));
        }
        crate::machine::rename(&node.world, &name)
            .map_err(|error| Failure::new(format!("renaming this computer to {name}: {error}")))?;
        node.set_machine(name.clone());
    }
    match node.link_of(&hub) {
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
            };
            let net = net_dir(&node.world, &hub);
            net.prepare()
                .and_then(|()| fastctx_net::link::state::write(&net.membership(), &membership))
                .map_err(|error| Failure::new(format!("logged in, but recording the membership failed: {error}")))?;
            start(node, membership).map_err(|error| Failure::new(format!("starting the link to the hub: {error}")))?;
            crate::ledger::joined(node, &hub);
        }
    }
    node.log.line(format_args!("logged in to {account} on {hub_name} as {name}"));
    Ok(Reply::LoggedIn { hub: hub_name, name, account })
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
        change => link.account_change(account, change).await.map_err(Failure::new),
    }
}

/// Logs this computer out of `account`; with no login left on that hub,
/// its membership is put aside and the link stops.
async fn log_out(node: &Arc<Node>, link: &Arc<Link>, account: &AccountName) -> Result<(), Failure> {
    let last = link.view().me().is_none_or(|me| me.logins.iter().all(|login| login == account));
    let answer = link.manage(account, Change::Logout).await;
    if !last {
        return answer.map(|_| ()).map_err(Failure::new);
    }
    // The hub ends the session of a member with no login left, and may do
    // so before its answer is out: being stopped is the answer then.
    if answer.is_err() && !matches!(link.status(), Status::Stopped { .. }) {
        answer.map_err(Failure::new)?;
    }
    let hub = link.membership().id.clone();
    link.stop();
    node.links.write().expect("never poisoned").remove(&hub);
    let net = net_dir(&node.world, &hub);
    let aside = node.world.path().join(format!("net-left-{}-{}", hub, Timestamp::now().strftime("%Y%m%dT%H%M%SZ")));
    std::fs::rename(net.path(), &aside)
        .map_err(|error| Failure::new(format!("putting {} aside: {error}", net.path().display())))?;
    node.log.line(format_args!("logged out of the hub {}; its records are in {}", link.hub_name(), aside.display()));
    Ok(())
}

/// Every hub this computer is on, as its views show them.
pub fn accounts(node: &Node) -> AccountsReply {
    let hubs = node
        .links()
        .iter()
        .map(|link| {
            let mut accounts = link.accounts();
            accounts.arrivals = crate::admitted::fresh(node, &accounts.hub);
            accounts
        })
        .collect();
    AccountsReply { machine: node.machine(), hubs }
}

/// What this machine is like.
pub fn facts(node: &Node) -> MachineFacts {
    let home =
        fastctx_platform::user_home().map(|path| native(&path)).unwrap_or_else(|error| format!("(not known: {error})"));
    MachineFacts {
        name: node.machine(),
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        home,
        temp: native(&std::env::temp_dir()),
        path_separator: if cfg!(windows) { ';' } else { ':' },
        case_sensitive: !cfg!(any(windows, target_os = "macos")),
        version: env!("CARGO_PKG_VERSION").to_string(),
        completion_receipts: true,
        shell: shell().clone(),
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

/// The bash `run` starts here and its version, asked once: the answer only
/// changes when the machine's bash is reinstalled, which a node restart
/// follows.
fn shell() -> &'static String {
    static SHELL: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    SHELL.get_or_init(|| {
        let bash = match fastctx_hands::run::shell::bash() {
            Ok(bash) => bash,
            Err(error) => return format!("no bash: {error}"),
        };
        let version = std::process::Command::new(&bash)
            .arg("--version")
            .stdin(std::process::Stdio::null())
            .output()
            .ok()
            .and_then(|output| {
                // "GNU bash, version 5.2.37(1)-release (x86_64-pc-msys)"
                let text = String::from_utf8_lossy(&output.stdout).into_owned();
                let rest = text.lines().next()?.split("version ").nth(1)?.to_string();
                Some(rest.split(['(', ' ']).next()?.to_string())
            });
        let version = version.map(|version| format!("bash {version}")).unwrap_or_else(|| "bash".to_string());
        if cfg!(windows) { format!("{version} (Git for Windows, MSYS)") } else { version }
    })
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
    fn answering(&self, call: &RemoteCall) -> Result<Option<Box<dyn Send>>, Failure> {
        // A watch and an `on` go on for as long as they go on: a node
        // handing the World on does not wait for them.
        if matches!(
            call,
            RemoteCall::Watch { .. }
                | RemoteCall::On { .. }
                | RemoteCall::Follow { .. }
                | RemoteCall::PipeOut { .. }
                | RemoteCall::FindFiles { request: fastctx_types::file_search::FileSearchRequest::Open { .. } }
        ) {
            return Ok(None);
        }
        let node = self.node.upgrade().ok_or_else(|| Failure::new("the node is stopping"))?;
        node.handoff.call().map(|guard| Some(Box::new(guard) as Box<dyn Send>)).ok_or_else(|| {
            Failure::new(format!(
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
        Box::pin(async move {
            let Some(node) = node else {
                return Err(Failure::new("the node is stopping"));
            };
            let link = node.link_of(&hub).ok_or_else(|| Failure::new("the link that brought the call is gone"))?;
            let asker = crate::admission::admit(&node, &link, &caller, &call)?;
            let from = caller.machine.clone();
            let acting = Acting::one(hub.clone(), caller.account.clone());
            let machine = node.machine();
            match call {
                RemoteCall::PageAction { source, action } => crate::pages::actions::answer(&node, source, action)
                    .await
                    .map(|result| RemoteReply::PageAction { result }),
                RemoteCall::Relay { made } => crate::devices::relay(&node, &asker, made).await,
                // A resident of the calling machine works here (`world-run.md`,
                // 这里).
                RemoteCall::Run { resident, town, request } => {
                    let account = HubAccount { hub: hub.clone(), account: caller.account.clone() };
                    crate::here::serve(&node, from, account, resident, town, request).await
                }
                RemoteCall::WordsCame { resident } => Ok(crate::here::words_came(&node, from, resident)),
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
                        crate::retention::received_from(&node, &owner, &delivery, origin.as_ref(), through_exit, Some(&account))
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
                RemoteCall::On { mut origin, sizes, writes_off, piped, source } => {
                    let stream = stream.ok_or_else(|| Failure::new("an on comes with a data connection"))?;
                    origin.follower = Some(from.clone());
                    origin.account = Some(HubAccount { hub: hub.clone(), account: caller.account.clone() });
                    let how = crate::on::Sent { writes_off, piped, source };
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
                RemoteCall::Follow { origin, job, out_from, err_from } => {
                    let stream = stream.ok_or_else(|| Failure::new("following an on needs a data connection"))?;
                    crate::on::serve_follow(&node, origin, job, out_from, err_from, stream).await
                }
                RemoteCall::StopOn { origin, job } => crate::on_stop::serve(&node, origin, job).await,
                RemoteCall::StopJob { job, by } => {
                    crate::stop::here_(&node, &job, &format!("{by} on {from}, for {}", caller.account))
                        .await
                        .map(|record| RemoteReply::JobStopped { record: record.map(Box::new) })
                }
                RemoteCall::Ask { asker: resident, town, label, prompt, started } => {
                    let acting = Acting::one(hub.clone(), caller.account.clone());
                    let asker = Address { machine: from, resident };
                    let colleague =
                        crate::colleagues::ask_here(&node, asker, acting, town, label, prompt, started).await?;
                    Ok(RemoteReply::Asked { colleague })
                }
                RemoteCall::Talk { to, text, attachments, files_on } => {
                    crate::talk::talk_here(&node, &from, &to, text, attachments, files_on.as_ref())
                        .await
                        .map(|landed| RemoteReply::Said { landed })
                }
                RemoteCall::Watch { resident, watch, composer } => {
                    let stream = stream
                        .ok_or_else(|| Failure::new("a watch comes with a data connection, and this one had none"))?;
                    crate::talk::serve_watch(&node, &link, resident, watch, composer, stream).await
                }
                RemoteCall::Agents => crate::agents::here(&node)
                    .await
                    .map(|reply| RemoteReply::Agents(crate::admission::seen::agents(&link.view(), &asker, reply))),
                RemoteCall::ClaudeConsent { accept, home } => {
                    crate::consent::here(&node, accept, home, format!("the account {} from {from}", caller.account))
                        .await
                        .map(|(accepted, changed)| RemoteReply::ClaudeConsent { accepted, changed })
                }
                RemoteCall::Homes { change } => crate::homes::change(&node, change).map(|()| RemoteReply::Set),
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
                RemoteCall::Open {
                    agent,
                    town,
                    text,
                    model,
                    reasoning_effort,
                    attachments,
                    works_on,
                    home,
                    files_on,
                } => {
                    let options = fastctx_types::agents::SessionOptions { model, reasoning_effort };
                    let choices = fastctx_types::agents::Choices { account: None, works_on, home, files_on };
                    let (resident, landed) =
                        crate::talk::open_here(&node, &from, acting, agent, town, text, options, attachments, choices)
                            .await?;
                    Ok(RemoteReply::Opened { resident, landed })
                }
                RemoteCall::WorksOn { resident, works_on } => {
                    crate::here::set(&node, &resident, works_on).await.map(|()| RemoteReply::Set)
                }
                RemoteCall::Stop { resident } => {
                    crate::talk::stop_here(&node, &resident).await.map(|stopped| RemoteReply::Stopped { stopped })
                }
                RemoteCall::Manage { resident, action } => {
                    crate::management::here(&node, &resident, action).await.map(RemoteReply::Managed)
                }
                RemoteCall::Control { resident, control } => {
                    crate::control::control_here(&node, &resident, control).await.map(RemoteReply::Controlled)
                }
                RemoteCall::Composer { resident, request } => crate::composer::here(&node, &from, &resident, request)
                    .await
                    .map(|updated| RemoteReply::Composer(Box::new(updated))),
                RemoteCall::Offers { resident } => crate::control::offers_here(&node, &resident)
                    .await
                    .map(|offers| RemoteReply::Offers(Box::new(offers))),
                RemoteCall::Look { id } => {
                    let (looked, mut jobs) = crate::look::look_here(&node, &id).await?;
                    jobs.retain(|job| crate::admission::seen::job(&node, &asker, job));
                    Ok(RemoteReply::Looked { looked: crate::admission::seen::looked(&node, &asker, looked), jobs })
                }
                RemoteCall::Find { resident } => {
                    let lives_here = crate::admission::seen::resident(&node, &asker, &resident);
                    Ok(RemoteReply::Found { lives_here })
                }
                RemoteCall::Who { all, town, since } => crate::server::who_here(&node, all, town, since)
                    .await
                    .map(|reply| RemoteReply::Who(crate::admission::seen::who(&node, &asker, reply))),
                RemoteCall::Since { since, town } => crate::since::here(&node, since, town)
                    .await
                    .map(|reply| RemoteReply::Since(crate::admission::seen::since(&node, &asker, reply))),
                RemoteCall::Canon { path } => {
                    tokio::task::spawn_blocking(move || crate::town::map::heads(std::path::Path::new(&path)))
                        .await
                        .map_err(|error| Failure::new(error.to_string()))?
                        .map(|heads| RemoteReply::Canon { heads })
                        .map_err(|error| Failure::new(format!("reading the canon on {machine}: {error}")))
                }
                RemoteCall::Facts => Ok(RemoteReply::Facts(facts(&node))),
                RemoteCall::CopySend { from: path, to } => {
                    let from = fastctx_types::copy::Place { machine: None, hub: None, path };
                    Box::pin(crate::copy::copy(&node, &acting, from, to)).await.map(RemoteReply::Copied)
                }
                RemoteCall::CopyFetch { from: path } => {
                    let stream = stream
                        .ok_or_else(|| Failure::new("a copy comes with a data connection, and this one had none"))?;
                    crate::copy::give(&node, &path, stream).await.map(RemoteReply::Copied)
                }
                RemoteCall::CopyReceive { to, manifest_bytes, manifest_sha256 } => {
                    let stream = stream
                        .ok_or_else(|| Failure::new("a copy comes with a data connection, and this one had none"))?;
                    crate::copy::receive(&node, to, manifest_bytes, manifest_sha256, stream)
                        .await
                        .map(RemoteReply::Copied)
                }
                RemoteCall::Place { resident, attachment } => {
                    let stream = stream
                        .ok_or_else(|| Failure::new("a file comes with a data connection, and this one had none"))?;
                    crate::attachments::take_placed(&node, &from, resident, attachment, stream)
                        .await
                        .map(RemoteReply::Uploaded)
                }
                RemoteCall::Upload { target, file, composer } => {
                    let stream = stream.ok_or_else(|| Failure::new("an upload requires its data connection"))?;
                    crate::attachments::receive(&node, &from, target, file, composer, stream)
                        .await
                        .map(crate::attachments::remote_reply)
                }
            }
        })
    }

    fn letter(
        &self,
        caller: Caller,
        purpose: Purpose,
        id: LetterId,
        letter: Letter,
    ) -> BoxFuture<io::Result<Result<(), String>>> {
        let from = caller.machine.clone();
        // What the letter carries is named here from who sent it and what
        // they named it, never by a name from the letter itself: a letter
        // that comes again is the same thing, and is not told twice.
        let delivery_id = crate::speech::fixed_id("letter", &[from.as_str(), id.as_str()]);
        let node = self.node.upgrade();
        let hub = self.hub.clone();
        Box::pin(async move {
            let Some(node) = node else {
                return Err(io::Error::other("the node is stopping"));
            };
            let _answering = node.handoff.call().ok_or_else(|| io::Error::other("the node is handing over"))?;
            let (letter, delivery_id) = match letter {
                Letter::Fragment { id, index, total, text } => {
                    match crate::letters::receive(&node, &from, &id, index, total, &text)? {
                        Some(letter) => (letter, crate::speech::fixed_id("letter", &[from.as_str(), id.as_str()])),
                        None => return Ok(Ok(())),
                    }
                }
                letter => (letter, delivery_id),
            };
            if let Err(refused) = crate::admission::letters::admit(&node, &hub, &caller, purpose, &letter) {
                return Ok(Err(refused));
            }
            match letter {
                Letter::Fragment { .. } => unreachable!("nested fragments are rejected"),
                Letter::Notice { to, notice } => {
                    let notice = *notice;
                    if let Notice::JobEnded { job, machine, .. } = &notice {
                        crate::here::ended_elsewhere(&node, &to, machine, job);
                    }
                    let delivery = Delivery { id: delivery_id, at: Timestamp::now(), notice };
                    if crate::on_delivery::park(&node, &to, &delivery)? {
                        return Ok(Ok(()));
                    }
                    match crate::speech::tell_here(&node, &to, delivery, Owed::Working).await {
                        Ok(_) => Ok(Ok(())),
                        Err(failure) => Err(io::Error::other(failure.message)),
                    }
                }
                Letter::Speak { to, from: speaker, text, .. } => {
                    let address = Address { machine: from, resident: speaker };
                    node.residents.set_departed(address.clone(), false);
                    let notice = Notice::Words { from: Speaker::Resident { address: address.clone() }, text };
                    let delivery = Delivery { id: delivery_id, at: Timestamp::now(), notice };
                    match crate::speech::tell_here(&node, &to, delivery, Owed::Speaker(address)).await {
                        Ok(_) => Ok(Ok(())),
                        Err(failure) => Err(io::Error::other(failure.message)),
                    }
                }
                Letter::Left { resident } => {
                    crate::colleagues::departed(&node, Address { machine: from, resident });
                    Ok(Ok(()))
                }
            }
        })
    }

    fn ledger_changed(&self, town: Option<fastctx_types::access::TownId>) {
        let Some(node) = self.node.upgrade() else { return };
        let hub = self.hub.clone();
        tokio::spawn(async move {
            // Connected again: the projects this computer kept on no hub go
            // into its account first.
            if town.is_none() {
                for problem in crate::ledger::carry(&node, &hub).await {
                    node.log.line(format_args!("carrying a project into the hub: {problem}"));
                }
            }
            if let Err(error) = crate::ledger::sync(&node, &hub, town).await {
                node.log.line(format_args!("taking the projects' ledgers: {error}"));
            }
        });
    }

    /// Someone may have logged in to an account this computer is logged in
    /// to: its user is told who, from the views before and after.
    fn directory_changed(&self) {
        let Some(node) = self.node.upgrade() else { return };
        crate::admitted::notice(&node, &self.hub);
        // The machines each session may use are in its tools' descriptions.
        node.tools_changed(None);
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
/// one machine. One whose agents alone it was lent is not among them.
pub fn machines(node: &Node, acting: &Acting) -> (MachineName, Vec<String>) {
    let here = node.machine();
    let mut others: Vec<String> = Vec::new();
    let mut keys = Vec::new();
    for link in node.links() {
        let view = link.view();
        let accounts = node.accounts_on(&link, acting);
        for machine in view.machines().filter(|machine| machine.name != here) {
            if keys.contains(&machine.key) {
                continue;
            }
            if accounts.iter().any(|account| view.rights(account, &machine.name).hands) {
                keys.push(machine.key);
                others.push(node.written(&link, &machine.name).to_string());
            }
        }
    }
    others.sort();
    (here, others)
}
