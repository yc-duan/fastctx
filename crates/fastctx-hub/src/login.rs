//! Logging in (`world-network.md`, 登录与入网): the hub's side of a password
//! login, a login with an enrollment token, and opening an account, each on
//! a connection of its own that closes once it is done.
//!
//! Wrong passwords are slowed per account and per address the login comes
//! from: after five in a row, each further try must wait, from one second,
//! doubling up to fifteen minutes; a login that succeeds clears both. Every
//! try counts as a failure until it succeeds, so tries made side by side
//! count too.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use fastctx_net::directory::{Account, Member};
use fastctx_net::login::HubSecrets;
use fastctx_net::sign::{Bytes, Domain, PublicKey, Signature};
use fastctx_net::wire::{Down, Joining, Refusal, SILENCE, Up};
use fastctx_types::machine_log::{Action, Actor, LoginMethod, Outcome};
use fastctx_types::{AccountName, MachineName};
use jiff::Timestamp;
use tokio::time::Instant;

use crate::store::Store;
use crate::{Hub, Socket, recv, refuse, send};

/// Tries allowed in a row before each must wait.
const FREE: u32 = 5;

/// The longest a try must wait.
const LONGEST: Duration = Duration::from_secs(15 * 60);

#[derive(Default)]
pub(crate) struct Limits {
    accounts: HashMap<AccountName, Strikes>,
    sources: HashMap<IpAddr, Strikes>,
}

#[derive(Clone, Copy)]
struct Strikes {
    failures: u32,
    last: Instant,
}

impl Strikes {
    /// How long after the last failure the next try may come.
    fn wait(&self) -> Duration {
        if self.failures < FREE {
            return Duration::ZERO;
        }
        let doublings = (self.failures - FREE).min(20);
        Duration::from_secs(1u64 << doublings).min(LONGEST)
    }

    fn remaining(&self) -> Duration {
        (self.last + self.wait()).saturating_duration_since(Instant::now())
    }
}

impl Limits {
    /// Starts a try for `account` from `source`: refused with how long to
    /// wait, or counted as a failure until it succeeds.
    fn begin(&mut self, account: &AccountName, source: IpAddr) -> Result<(), String> {
        let wait = [self.accounts.get(account), self.sources.get(&source)]
            .into_iter()
            .flatten()
            .map(Strikes::remaining)
            .max()
            .unwrap_or_default();
        if !wait.is_zero() {
            return Err(format!(
                "too many wrong passwords for this account or from this address; try again in {} s",
                wait.as_secs().max(1)
            ));
        }
        let now = Instant::now();
        for strikes in [
            self.accounts.entry(account.clone()).or_insert(Strikes { failures: 0, last: now }),
            self.sources.entry(source).or_insert(Strikes { failures: 0, last: now }),
        ] {
            strikes.failures += 1;
            strikes.last = now;
        }
        Ok(())
    }

    fn succeeded(&mut self, account: &AccountName, source: IpAddr) {
        self.accounts.remove(account);
        self.sources.remove(&source);
    }
}

/// A password login to `account`, after its first message.
pub(crate) async fn with_password(
    hub: Arc<Hub>,
    mut socket: Socket,
    peer: SocketAddr,
    binding: [u8; 32],
    account: AccountName,
    request: Bytes,
) {
    let begun = hub.limits.lock().expect("never poisoned").begin(&account, peer.ip());
    if let Err(reason) = begun {
        hub.log.line(format_args!("{peer}: a login to {account} waits: {reason}"));
        refuse(&mut socket, Refusal::Login { reason }).await;
        return;
    }
    let record = hub.store().book.passwords.get(&account).map(|record| record.0.clone());
    let (response, state) = match hub.secrets.login_start(&account, record.as_deref(), &request, &binding) {
        Ok(started) => started,
        Err(reason) => {
            hub.log.line(format_args!("{peer}: a login to {account} could not start: {reason}"));
            refuse(&mut socket, Refusal::Login { reason }).await;
            return;
        }
    };
    if send(&mut socket, &Down::LoginResponse { response }).await.is_err() {
        return;
    }
    let (finalization, member) = match tokio::time::timeout(SILENCE, recv(&mut socket)).await {
        Ok(Ok(Some(Up::LoginFinish { finalization, member }))) => (finalization, member),
        // The device found the hub's half did not open: a wrong password,
        // or a hub it was not talking to.
        Ok(Ok(None)) => {
            hub.log.line(format_args!("{peer}: a login to {account} was given up"));
            return;
        }
        _ => {
            let reason = "a login goes on with its last message".to_string();
            hub.log.line(format_args!("{peer}: a login to {account} did not finish: {reason}"));
            refuse(&mut socket, Refusal::Broken { reason }).await;
            return;
        }
    };
    if let Err(reason) = HubSecrets::login_finish(&state, &finalization) {
        hub.log.line(format_args!("{peer}: a login to {account} failed: {reason}"));
        refuse(&mut socket, Refusal::Login { reason }).await;
        return;
    }
    hub.limits.lock().expect("never poisoned").succeeded(&account, peer.ip());
    let outcome = proved(&member, &binding).and_then(|()| join(&hub, peer, &account, &member, member.register, None));
    logged_in(&hub, &mut socket, peer, &account, outcome).await;
}

/// A login with the enrollment token whose key is `token`.
pub(crate) async fn with_token(
    hub: Arc<Hub>,
    mut socket: Socket,
    peer: SocketAddr,
    binding: [u8; 32],
    token: PublicKey,
    proof: Signature,
    member: Joining,
) {
    let outcome = (|| {
        token
            .verify(Domain::Session, &binding, &proof)
            .map_err(|_| "the proof of the token does not verify".to_string())?;
        proved(&member, &binding)?;
        let record = {
            let store = hub.store();
            store.book.tokens.iter().find(|record| record.key == token).cloned()
        };
        let record = record.ok_or("the hub knows no such token; it may have been taken back")?;
        let refused = match record.used_by {
            Some(used) if used != member.key => Some("the token was used already"),
            None if record.expires <= Timestamp::now() => Some("the token has expired"),
            _ => None,
        };
        if let Some(reason) = refused {
            let mut store = hub.store();
            let directory = store.book.directory.clone();
            crate::machine_log::keep(
                &mut store,
                &directory,
                Actor {
                    account: Some(record.account.clone()),
                    authenticated: directory
                        .by_key(&member.key)
                        .is_some_and(|known| known.logins.contains(&record.account)),
                    member: directory.by_key(&member.key).map(|member| member.name.clone()),
                    peer: Some(peer),
                },
                Action::Joined {
                    name: record.name.clone().unwrap_or_else(|| member.name.clone()),
                    kind: record.kind,
                    registered: record.register,
                    via: LoginMethod::Token,
                },
                Outcome::Refused { reason: reason.to_string() },
            )?;
            hub.machine_log_changed.notify_one();
            return Err(reason.to_string());
        }
        let joining = Joining {
            name: record.name.clone().unwrap_or_else(|| member.name.clone()),
            kind: record.kind,
            ..member.clone()
        };
        let name = join(&hub, peer, &record.account, &joining, record.register, Some(&token))?;
        Ok((record.account, name))
    })();
    match outcome {
        Ok((account, name)) => logged_in(&hub, &mut socket, peer, &account, Ok(name)).await,
        Err(reason) => {
            hub.log.line(format_args!("{peer}: a token login was refused: {reason}"));
            refuse(&mut socket, Refusal::Login { reason }).await;
        }
    }
}

/// Opening `account` on a hub that lets anyone, after its first message.
pub(crate) async fn open_account(
    hub: Arc<Hub>,
    mut socket: Socket,
    peer: SocketAddr,
    account: AccountName,
    request: Bytes,
) {
    let refused = {
        let store = hub.store();
        if !store.hub.self_registration {
            Some("this hub opens accounts only through its super administrator".to_string())
        } else if store.book.directory.account(&account).is_some() {
            Some(format!("the account {account} exists already"))
        } else {
            None
        }
    };
    if let Some(reason) = refused {
        let reason =
            record_opening(&hub, peer, &account, Outcome::Refused { reason: reason.clone() }).err().unwrap_or(reason);
        refuse(&mut socket, Refusal::Login { reason }).await;
        return;
    }
    let response = match hub.secrets.register_start(&account, &request) {
        Ok(response) => response,
        Err(reason) => {
            let reason = record_opening(&hub, peer, &account, Outcome::Refused { reason: reason.clone() })
                .err()
                .unwrap_or(reason);
            refuse(&mut socket, Refusal::Login { reason }).await;
            return;
        }
    };
    if send(&mut socket, &Down::RegisterResponse { response }).await.is_err() {
        return;
    }
    let upload = match tokio::time::timeout(SILENCE, recv(&mut socket)).await {
        Ok(Ok(Some(Up::RegisterFinish { upload }))) => upload,
        Ok(Ok(None)) => return,
        _ => {
            refuse(
                &mut socket,
                Refusal::Broken { reason: "opening an account goes on with its last message".to_string() },
            )
            .await;
            return;
        }
    };
    let outcome = HubSecrets::register_finish(&upload).and_then(|record| {
        let mut store = hub.store();
        let before = store.book.clone();
        if store.book.directory.account(&account).is_some() {
            return Err(format!("the account {account} exists already"));
        }
        store.book.directory.accounts.push(Account {
            name: account.clone(),
            level: fastctx_net::directory::Level::Member,
            locked: false,
            opened: Timestamp::now(),
            collab: None,
        });
        store.book.passwords.insert(account.clone(), Bytes(record));
        let actor = Actor { account: None, authenticated: false, member: None, peer: Some(peer) };
        if let Err(reason) = crate::machine_log::keep(
            &mut store,
            &before.directory,
            actor,
            Action::OpenAccount { name: account.clone() },
            Outcome::Done,
        ) {
            store.book = before;
            return Err(reason);
        }
        Ok(())
    });
    match outcome {
        Ok(()) => {
            hub.log.line(format_args!("{peer}: opened the account {account}"));
            let _ = send(&mut socket, &Down::Registered).await;
            let _ = futures_util::SinkExt::close(&mut socket).await;
            hub.changed();
        }
        Err(reason) => {
            let reason = record_opening(&hub, peer, &account, Outcome::Refused { reason: reason.clone() })
                .err()
                .unwrap_or(reason);
            refuse(&mut socket, Refusal::Login { reason }).await;
        }
    }
}

fn record_opening(hub: &Hub, peer: SocketAddr, account: &AccountName, outcome: Outcome) -> Result<(), String> {
    let mut store = hub.store();
    let directory = store.book.directory.clone();
    let result = crate::machine_log::keep(
        &mut store,
        &directory,
        Actor { account: None, authenticated: false, member: None, peer: Some(peer) },
        Action::OpenAccount { name: account.clone() },
        outcome,
    );
    hub.machine_log_changed.notify_one();
    result
}

/// That `asked` is taken: by a name written as it is, or written another
/// way, as `GPU-2` takes `gpu-2`.
pub(crate) fn taken_by(asked: &MachineName, taken: &MachineName) -> String {
    if asked.as_str() == taken.as_str() {
        format!("the name {asked} is taken on this hub")
    } else {
        format!("the name {asked} is taken on this hub, written {taken}")
    }
}

/// Why `asked` cannot be a new name on the hub, if it cannot: another
/// member than `except` has it, or had it before it was renamed, and it
/// still names that member.
pub(crate) fn name_taken(
    directory: &fastctx_net::directory::Directory,
    asked: &MachineName,
    except: Option<&PublicKey>,
) -> Option<String> {
    let holder = directory.member(asked).filter(|member| Some(&member.key) != except)?;
    Some(if holder.name == *asked {
        taken_by(asked, &holder.name)
    } else {
        format!("the name {asked} was {}'s before it was renamed, and still names it", holder.name)
    })
}

/// What a device of a release from before names of any language is told
/// when the hub has such a name.
pub(crate) const OLDER_RELEASE: &str = "this hub has names beyond lowercase English letters, digits and hyphens, which this release of fastctx cannot read; update fastctx";

/// Whether the hub or one of its members has a name that a release from
/// before names of any language cannot read.
pub(crate) fn beyond_older_releases(store: &Store) -> bool {
    !store.hub.name.is_ascii_word() || store.book.directory.members.iter().any(|member| !member.name.is_ascii_word())
}

/// Names of any language reach every member in what the hub sends it, and a
/// release from before them cannot read such a name: it would lose its hub
/// over it. So a device of such a release comes in only while every name on
/// the hub is one it reads, and a name it cannot read comes in only while
/// every other member's last session said it reads them.
fn readable(store: &Store, joining: &Joining) -> Result<(), String> {
    if !joining.any_names && beyond_older_releases(store) {
        return Err(format!("{OLDER_RELEASE} and log in again"));
    }
    let directory = &store.book.directory;
    let name = directory.by_key(&joining.key).map_or(&joining.name, |member| &member.name);
    match unread(store, name, Some(&joining.key)) {
        Some(reason) => Err(reason),
        None => Ok(()),
    }
}

/// Why `name` cannot come into the hub now, if it cannot: members other
/// than `except` whose last session did not say they read it.
pub(crate) fn unread(store: &Store, name: &MachineName, except: Option<&PublicKey>) -> Option<String> {
    if name.is_ascii_word() {
        return None;
    }
    let older: Vec<String> = store
        .book
        .directory
        .members
        .iter()
        .filter(|member| Some(&member.key) != except)
        .filter(|member| !store.letters.get(&member.name).is_some_and(|letters| letters.any_names))
        .map(|member| member.name.to_string())
        .collect();
    (!older.is_empty()).then(|| {
        format!(
            "the name {name} goes beyond lowercase English letters, digits and hyphens, and {} last connected with a release of fastctx that cannot read such a name; update fastctx there, or choose a name of lowercase English letters, digits and hyphens",
            older.join(", ")
        )
    })
}

/// Whether the joining device holds the key it names, on this connection.
fn proved(member: &Joining, binding: &[u8; 32]) -> Result<(), String> {
    member
        .key
        .verify(Domain::Session, binding, &member.proof)
        .map_err(|_| "the proof of the device's key does not verify".to_string())
}

/// Logs the device `joining` in to `account`, registering it as one of the
/// account's machines when `register` and the account may; the token used,
/// if any, is marked used by it. Returns its name on the hub.
fn join(
    hub: &Hub,
    peer: SocketAddr,
    account: &AccountName,
    joining: &Joining,
    register: bool,
    token: Option<&PublicKey>,
) -> Result<fastctx_types::MachineName, String> {
    let name = {
        let mut store = hub.store();
        let before = store.book.clone();
        let result = (|| {
            readable(&store, joining)?;
            let found = store.book.directory.account(account).cloned();
            let found = found.ok_or_else(|| format!("there is no account {account}"))?;
            if found.locked {
                return Err(format!("the account {account} is locked"));
            }
            let directory = &mut store.book.directory;
            let name = match directory.members.iter_mut().find(|member| member.key == joining.key) {
                Some(member) => {
                    member.logins.insert(account.clone());
                    if register && member.home.is_none() && member.kind.is_computer() {
                        member.home = Some(account.clone());
                    }
                    member.name.clone()
                }
                None => {
                    if let Some(taken) = name_taken(directory, &joining.name, None) {
                        return Err(format!("{taken}; log in under another"));
                    }
                    let home = (register && joining.kind.is_computer()).then(|| account.clone());
                    directory.members.push(Member {
                        name: joining.name.clone(),
                        was: Vec::new(),
                        key: joining.key,
                        kind: joining.kind,
                        logins: [account.clone()].into(),
                        home,
                        direct: Vec::new(),
                        added: Timestamp::now(),
                        online: false,
                        since: None,
                        through: None,
                        reach: None,
                        paths: None,
                    });
                    joining.name.clone()
                }
            };
            store.book.removed.retain(|removed| removed.key != joining.key);
            if let Some(token) = token
                && let Some(record) = store.book.tokens.iter_mut().find(|record| record.key == *token)
            {
                record.used_by = Some(joining.key);
            }
            Ok(name)
        })();
        let (name, outcome) = match &result {
            Ok(name) => (name.clone(), Outcome::Done),
            Err(reason) => {
                store.book = before.clone();
                (joining.name.clone(), Outcome::Refused { reason: reason.clone() })
            }
        };
        let actor = Actor {
            account: Some(account.clone()),
            authenticated: true,
            member: store.book.directory.by_key(&joining.key).map(|member| member.name.clone()),
            peer: Some(peer),
        };
        let registered = store.book.directory.member(&name).is_some_and(|member| member.home.as_ref() == Some(account));
        let action = Action::Joined {
            name: name.clone(),
            kind: store.book.directory.member(&name).map_or(joining.kind, |member| member.kind),
            registered,
            via: if token.is_some() { LoginMethod::Token } else { LoginMethod::Password },
        };
        // Reconnecting a control session never reaches this path. A repeated
        // password login with no membership change is not a new arrival.
        if token.is_some() || result.is_err() || before.directory != store.book.directory {
            let mut scope = before.directory.clone();
            if let Some(member) = store.book.directory.member(&name) {
                scope.members.retain(|current| current.key != member.key);
                scope.members.push(member.clone());
            }
            if let Err(reason) = crate::machine_log::keep(&mut store, &scope, actor, action, outcome) {
                store.book = before;
                return Err(reason);
            }
            hub.machine_log_changed.notify_one();
        }
        result?
    };
    hub.changed();
    Ok(name)
}

async fn logged_in(
    hub: &Hub,
    socket: &mut Socket,
    peer: SocketAddr,
    account: &AccountName,
    outcome: Result<fastctx_types::MachineName, String>,
) {
    match outcome {
        Ok(name) => {
            hub.log.line(format_args!("{peer}: {name} logged in to {account}"));
            let (world, hub_name) = {
                let store = hub.store();
                (store.hub.id.clone(), store.hub.name.clone())
            };
            let _ = send(socket, &Down::LoggedIn { hub: world, hub_name, name, account: account.clone() }).await;
            let _ = futures_util::SinkExt::close(socket).await;
        }
        Err(reason) => {
            hub.log.line(format_args!("{peer}: a login to {account} was refused: {reason}"));
            refuse(socket, Refusal::Login { reason }).await;
        }
    }
}
