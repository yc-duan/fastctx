//! This machine among others: its key, its membership of a World, and its
//! link to the hub (`world-network.md`).
//!
//! A machine that has joined no World is a World of one: it has no key and
//! no link, and everything else works the same. Joining makes its key,
//! records the membership and starts the link; leaving revokes its own key
//! and puts the membership aside.

use std::io;
use std::sync::{Arc, Weak};

use fastctx_net::code::Code;
use fastctx_net::link::{BoxFuture, Link, Membership, NetDir, Status};
use fastctx_net::message::LetterId;
use fastctx_net::sign::SecretKey;
use fastctx_types::ipc::Failure;
use fastctx_types::layout::WorldDir;
use fastctx_types::remote::{Letter, RemoteCall, RemoteReply, RemoteResult};
use fastctx_types::residents::{Delivery, Notice};
use fastctx_types::session::Speaker;
use fastctx_types::world::{LinkState, MachineFacts};
use fastctx_types::{Address, MachineName, TownName};
use jiff::Timestamp;

use crate::Node;
use crate::speech::Owed;

/// The membership and key this machine joined with, if it has joined.
pub fn load(world: &WorldDir) -> io::Result<Option<(Membership, SecretKey)>> {
    let net = NetDir::new(world.net());
    let membership: Membership = match fastctx_net::link::state::read(&net.membership()) {
        Ok(membership) => membership,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let key = SecretKey::load(&world.device_key())?;
    Ok(Some((membership, key)))
}

/// Starts the link of a member.
pub fn start(node: &Arc<Node>, membership: Membership, key: SecretKey) -> io::Result<Arc<Link>> {
    let handler = Arc::new(Handler { node: Arc::downgrade(node) });
    let link = Arc::new(Link::start(NetDir::new(node.world.net()), membership, key, handler)?);
    *node.link.write().expect("never poisoned") = Some(link.clone());
    Ok(link)
}

/// Joins the World `code` is for.
pub async fn join(node: &Arc<Node>, code: &str, name: Option<MachineName>) -> Result<(String, MachineName), Failure> {
    if let Some(link) = node.link() {
        return Err(Failure::new(format!(
            "this machine is {} in the World {} already; `fastctx leave` first to join another",
            link.membership().name,
            link.membership().world
        )));
    }
    let code = Code::parse(code).map_err(Failure::new)?;
    let name = name.unwrap_or_else(|| node.machine());
    let world = node.world.clone();
    let key = tokio::task::spawn_blocking(move || crate::ledger::own_key(&world))
        .await
        .map_err(|error| Failure::new(format!("making this machine's key: {error}")))?
        .map_err(|error| Failure::new(format!("making this machine's key: {error}")))?;
    let membership = fastctx_net::join::join(&code, name.clone(), &key).await.map_err(Failure::new)?;
    let net = NetDir::new(node.world.net());
    let recorded = net
        .prepare()
        .and_then(|()| fastctx_net::link::state::write(&net.membership(), &membership))
        .and_then(|()| crate::machine::rename(&node.world, &name));
    recorded
        .map_err(|error| Failure::new(format!("the hub admitted this machine, but recording that failed: {error}")))?;
    node.set_machine(name.clone());
    let world = membership.world.to_string();
    start(node, membership, key).map_err(|error| Failure::new(format!("starting the link to the hub: {error}")))?;
    Ok((world, name))
}

/// Leaves the World: revokes this machine's own key, when the hub can be
/// told, and puts the membership aside.
pub async fn leave(node: &Arc<Node>) -> Result<(), Failure> {
    let Some(link) = node.link() else {
        return Err(Failure::new("this machine is in no World with others"));
    };
    let me = link.membership().name.clone();
    if !matches!(link.status(), Status::Stopped { .. }) {
        link.revoke(&me, "left").await.map_err(|error| Failure::new(format!("telling the hub: {error}")))?;
    }
    link.stop();
    *node.link.write().expect("never poisoned") = None;
    let net = node.world.net();
    let aside = node.world.path().join(format!("net-left-{}", jiff::Timestamp::now().strftime("%Y%m%dT%H%M%SZ")));
    std::fs::rename(&net, &aside).map_err(|error| Failure::new(format!("putting {} aside: {error}", net.display())))?;
    node.log.line(format_args!("left the World; its records are in {}", aside.display()));
    Ok(())
}

pub fn link_state(link: Option<&Link>) -> LinkState {
    match link.map(Link::status) {
        None => LinkState::Alone,
        Some(Status::Connecting { since, last_error }) => LinkState::Connecting { since, last_error },
        Some(Status::Online { since, address, route }) => LinkState::Online { since, hub: address.to_string(), route },
        Some(Status::Stopped { reason }) => LinkState::Stopped { reason },
    }
}

/// What this machine is like.
pub fn facts(machine: &MachineName) -> MachineFacts {
    let home =
        fastctx_platform::user_home().map(|path| native(&path)).unwrap_or_else(|error| format!("(not known: {error})"));
    MachineFacts {
        name: machine.clone(),
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        home,
        temp: native(&std::env::temp_dir()),
        path_separator: if cfg!(windows) { ';' } else { ':' },
        case_sensitive: !cfg!(any(windows, target_os = "macos")),
        version: env!("CARGO_PKG_VERSION").to_string(),
    }
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

/// What the link hands this node.
struct Handler {
    node: Weak<Node>,
}

impl fastctx_net::link::Node for Handler {
    fn answering(&self, call: &RemoteCall) -> Result<Option<Box<dyn Send>>, Failure> {
        if matches!(call, RemoteCall::Watch { .. }) {
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
        from: MachineName,
        call: RemoteCall,
        stream: Option<fastctx_net::data::Stream>,
    ) -> BoxFuture<RemoteResult> {
        let node = self.node.upgrade();
        Box::pin(async move {
            let Some(node) = node else {
                return Err(Failure::new("the node is stopping"));
            };
            let machine = node.machine();
            crate::devices::admit(&node, &from, &call)?;
            match call {
                RemoteCall::PageAction { source, action } => crate::pages::actions::execute(&node, source, action)
                    .await
                    .map(|result| RemoteReply::PageAction { result }),
                RemoteCall::Relay { made } => crate::devices::relay(&node, &from, made).await,
                RemoteCall::Run { resident, town, request } => {
                    let owner = Address { machine: from, resident };
                    let (reply, _) = node.jobs.run(&node, owner.into(), town, request).await?;
                    Ok(RemoteReply::Run(reply))
                }
                RemoteCall::Ask { asker, town, label, prompt } => {
                    let asker = Address { machine: from, resident: asker };
                    let colleague = crate::colleagues::ask_here(&node, asker, town, label, prompt).await?;
                    Ok(RemoteReply::Asked { colleague })
                }
                RemoteCall::Talk { to, text, attachments } => {
                    crate::talk::talk_here(&node, &from, &to, text, attachments)
                        .await
                        .map(|landed| RemoteReply::Said { landed })
                }
                RemoteCall::Watch { resident, watch } => {
                    let stream = stream
                        .ok_or_else(|| Failure::new("a watch comes with a data connection, and this one had none"))?;
                    crate::talk::serve_watch(&node, resident, watch, stream).await
                }
                RemoteCall::Agents => crate::agents::here(&node).await.map(RemoteReply::Agents),
                RemoteCall::ClaudeConsent { accept } => {
                    crate::consent::here(&node, accept, format!("the user on {from}"))
                        .await
                        .map(|(accepted, changed)| RemoteReply::ClaudeConsent { accepted, changed })
                }
                RemoteCall::FindFiles { request } => {
                    crate::file_search::here(&node, from, request).await.map(RemoteReply::FilesFound)
                }
                RemoteCall::ReadFile { request, read } => {
                    let stream = stream.ok_or_else(|| Failure::new("a file read requires a data connection"))?;
                    crate::file_read::serve(&node, request, read, stream).await
                }

                RemoteCall::Open { agent, town, text, model, reasoning_effort, attachments } => {
                    let options = fastctx_types::agents::SessionOptions { model, reasoning_effort };
                    let (resident, landed) =
                        crate::talk::open_here(&node, &from, agent, town, text, options, attachments).await?;
                    Ok(RemoteReply::Opened { resident, landed })
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
                RemoteCall::Offers { resident } => crate::control::offers_here(&node, &resident)
                    .await
                    .map(|offers| RemoteReply::Offers(Box::new(offers))),
                RemoteCall::Look { id } => Ok(RemoteReply::Looked { looked: crate::look::look_here(&node, &id).await }),
                RemoteCall::Find { resident } => {
                    Ok(RemoteReply::Found { lives_here: node.residents.town_of(&resident).is_some() })
                }
                RemoteCall::Write { resident, town, op } => {
                    let owner = Address { machine: from, resident };
                    let world = node.world.clone();
                    let outcome = tokio::task::spawn_blocking(move || {
                        crate::write::write_here(&world, &machine, &owner, town.as_ref(), op)
                    })
                    .await
                    .map_err(|error| Failure::new(format!("writing: {error}")))?
                    .map_err(|error| Failure::new(format!("writing: {error}")))?;
                    Ok(RemoteReply::Written { outcome })
                }
                RemoteCall::Who { all, town } => crate::server::who_here(&node, all, town).await.map(RemoteReply::Who),
                RemoteCall::Since { since, town } => {
                    crate::since::here(&node, since, town).await.map(RemoteReply::Since)
                }
                RemoteCall::Canon { path } => {
                    tokio::task::spawn_blocking(move || crate::town::map::heads(std::path::Path::new(&path)))
                        .await
                        .map_err(|error| Failure::new(error.to_string()))?
                        .map(|heads| RemoteReply::Canon { heads })
                        .map_err(|error| Failure::new(format!("reading the canon on {machine}: {error}")))
                }
                RemoteCall::Facts => Ok(RemoteReply::Facts(facts(&machine))),
                RemoteCall::CopySend { from, to } => {
                    let to_machine = to.machine.ok_or_else(|| Failure::new("the copy names no machine to send to"))?;
                    crate::copy::send(&node, &from, &to_machine, &to.path).await.map(RemoteReply::Copied)
                }
                RemoteCall::CopyReceive { to, manifest_bytes, manifest_sha256 } => {
                    let stream = stream
                        .ok_or_else(|| Failure::new("a copy comes with a data connection, and this one had none"))?;
                    crate::copy::receive(&node, to, manifest_bytes, manifest_sha256, stream)
                        .await
                        .map(RemoteReply::Copied)
                }
                RemoteCall::Upload { target, file } => {
                    let stream = stream.ok_or_else(|| Failure::new("an upload requires its data connection"))?;
                    crate::attachments::receive(&node, target, file, stream).await.map(RemoteReply::Uploaded)
                }
            }
        })
    }

    fn letter(&self, from: MachineName, id: LetterId, letter: Letter) -> BoxFuture<io::Result<Result<(), String>>> {
        // What the letter carries is named here from who sent it and what
        // they named it, never by a name from the letter itself: a letter
        // that comes again is the same thing, and is not told twice.
        let delivery_id = crate::speech::fixed_id("letter", &[from.as_str(), id.as_str()]);
        let node = self.node.upgrade();
        Box::pin(async move {
            let Some(node) = node else {
                return Err(io::Error::other("the node is stopping"));
            };
            let _answering = node.handoff.call().ok_or_else(|| io::Error::other("the node is handing over"))?;
            if crate::devices::is_device(&node, &from) {
                return Ok(Err(format!("{from} is a device, and a device sends no letters")));
            }
            let (letter, delivery_id) = match letter {
                Letter::Fragment { id, index, total, text } => {
                    match crate::letters::receive(&node, &from, &id, index, total, &text)? {
                        Some(letter) => (letter, crate::speech::fixed_id("letter", &[from.as_str(), id.as_str()])),
                        None => return Ok(Ok(())),
                    }
                }
                letter => (letter, delivery_id),
            };
            match letter {
                Letter::Fragment { .. } => unreachable!("nested fragments are rejected"),
                Letter::Notice { to, notice } => {
                    if node.residents.town_of(&to).is_none() {
                        return Ok(Err(format!(
                            "{from} wrote to {to}, and no resident {to} lives on {}",
                            node.machine()
                        )));
                    }
                    let delivery = Delivery { id: delivery_id, at: Timestamp::now(), notice };
                    match crate::speech::tell_here(&node, &to, delivery, Owed::Working).await {
                        Ok(_) => Ok(Ok(())),
                        Err(failure) => Err(io::Error::other(failure.message)),
                    }
                }
                Letter::Speak { to, from: speaker, text } => {
                    if node.residents.town_of(&to).is_none() {
                        return Ok(Err(format!(
                            "{from} wrote to {to}, and no resident {to} lives on {}",
                            node.machine()
                        )));
                    }
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

    fn ledger_changed(&self, town: Option<TownName>) {
        let Some(node) = self.node.upgrade() else { return };
        tokio::spawn(async move {
            if let Err(error) = crate::ledger::sync(&node, town).await {
                node.log.line(format_args!("taking the towns' ledgers: {error}"));
            }
        });
    }

    fn log(&self, line: String) {
        if let Some(node) = self.node.upgrade() {
            node.log.line(line);
        }
    }
}
