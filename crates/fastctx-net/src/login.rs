//! Logging in to a hub (`world-network.md`, 登录与入网): with an account's
//! password, or with a one-time enrollment token; and opening an account.
//!
//! A password login is OPAQUE (RFC 9807): the hub keeps only what the
//! password's registration left with it, never the password, and the login
//! proves to each side that the other knows its half. The login is bound to
//! the TLS connection it runs on: both sides put that connection's exporter
//! into the key exchange as its context, so a man in the middle, who holds
//! a different TLS connection with each side, makes the login fail instead
//! of learning anything. That is how a device that has never seen the hub
//! learns its certificate: it takes whichever the server presents for this
//! one connection, and keeps it only once the login succeeded.
//!
//! A token login dials the certificate the token names and proves the
//! token's key by signing the exporter.
//!
//! Every login sends what the device is: the name it asks for, its key and
//! a signature by that key of the exporter, its kind, and whether it also
//! registers as a machine of the account. A device logs in each time on a
//! connection of its own, which then closes; it connects again as a member
//! with its key (`crate::link`).

use std::net::SocketAddr;

use fastctx_types::{AccountName, HubName, MachineName};
use opaque_ke::{
    CipherSuite, ClientLogin, ClientLoginFinishParameters, ClientRegistration, ClientRegistrationFinishParameters,
    CredentialFinalization, CredentialRequest, CredentialResponse, RegistrationRequest, RegistrationResponse,
    RegistrationUpload, ServerLogin, ServerLoginStartParameters, ServerRegistration, ServerSetup,
};
use rand_core::OsRng;

use crate::dial::{self, HubSocket};
use crate::directory::Kind;
use crate::ids::{CertPin, HubId};
use crate::sign::{Bytes, Domain, PublicKey, SecretKey};
use crate::token::Token;
use crate::wire::{self, Down, Intent, Joining, SILENCE, Up};

/// The OPAQUE suite: ristretto255 for the OPRF and the key exchange,
/// 3DH, and Argon2id with its default parameters (19 MiB, two passes) to
/// stretch the password on the device.
pub struct Suite;

impl CipherSuite for Suite {
    type OprfCs = opaque_ke::Ristretto255;
    type KeGroup = opaque_ke::Ristretto255;
    type KeyExchange = opaque_ke::key_exchange::tripledh::TripleDh;
    type Ksf = argon2::Argon2<'static>;
}

/// What logged in: the hub, and this device as a member of it.
#[derive(Clone, Debug)]
pub struct Joined {
    pub hub: HubId,
    pub hub_name: HubName,
    pub name: MachineName,
    pub account: AccountName,
    /// The addresses the device reached the hub at.
    pub addresses: Vec<SocketAddr>,
    pub cert: CertPin,
}

/// The device logging in.
pub struct Device<'a> {
    pub name: MachineName,
    pub key: &'a SecretKey,
    pub kind: Kind,
    pub register: bool,
}

/// What a token lets a device into, as its hub told before it is used.
#[derive(Clone, Debug)]
pub struct Peeked {
    pub hub: HubId,
    pub hub_name: HubName,
    pub account: AccountName,
    pub managed: bool,
    pub label: Option<String>,
    pub expires: jiff::Timestamp,
}

impl Device<'_> {
    fn joining(&self, binding: &[u8; 32]) -> Joining {
        Joining {
            name: self.name.clone(),
            key: self.key.public(),
            proof: self.key.sign(Domain::Session, binding),
            kind: self.kind,
            register: self.register,
            any_names: true,
        }
    }
}

/// Logs in to `account` on the hub at `hub` with its password.
pub async fn with_password(
    hub: &[SocketAddr],
    account: &AccountName,
    password: &str,
    device: Device<'_>,
) -> Result<Joined, String> {
    let dialed = dial::dial(hub, None, wire::CONTROL_PATH, wire::MAX_CONTROL_FRAME).await?;
    let (mut socket, binding, presented) = (dialed.socket, dialed.binding, dialed.certificate);
    let start = ClientLogin::<Suite>::start(&mut OsRng, password.as_bytes())
        .map_err(|error| format!("starting the login: {error}"))?;
    let request = Bytes(start.message.serialize().to_vec());
    hello(&mut socket, Intent::Login { account: account.clone(), request }).await?;
    let response = match next(&mut socket).await? {
        Down::LoginResponse { response } => response,
        Down::Refused { reason } => return Err(reason.to_string()),
        _ => return Err("the hub answered the login with something else".to_string()),
    };
    let response = CredentialResponse::<Suite>::deserialize(&response.0)
        .map_err(|error| format!("the hub's answer to the login: {error}"))?;
    // A wrong password and a man in the middle look the same here: either
    // way the hub's half does not open with what this side holds.
    let finished = start
        .state
        .finish(password.as_bytes(), response, ClientLoginFinishParameters::new(Some(&binding), Default::default(), None))
        .map_err(|error| match error {
            opaque_ke::errors::ProtocolError::InvalidLoginError => {
                format!("the account {account} does not exist there or the password is wrong, or something between this computer and the hub is not the hub")
            }
            other => format!("finishing the login: {other}"),
        })?;
    let finish = Up::LoginFinish {
        finalization: Bytes(finished.message.serialize().to_vec()),
        member: device.joining(&binding),
    };
    dial::send(&mut socket, &finish).await?;
    joined(&mut socket, hub, CertPin::of(&presented)).await
}

/// Logs in with an enrollment token.
pub async fn with_token(token: &Token, device: Device<'_>) -> Result<Joined, String> {
    let dialed = dial::dial(&token.addresses, Some(token.cert), wire::CONTROL_PATH, wire::MAX_CONTROL_FRAME).await?;
    let (mut socket, binding) = (dialed.socket, dialed.binding);
    let key = token.secret.key();
    let intent = Intent::Token {
        token: key.public(),
        proof: key.sign(Domain::Session, &binding),
        member: device.joining(&binding),
    };
    hello(&mut socket, intent).await?;
    let joined = joined(&mut socket, &token.addresses, token.cert).await?;
    if joined.hub != token.hub {
        return Err(format!("the hub is {}, not {} the token names", joined.hub, token.hub));
    }
    Ok(joined)
}

/// What `token` lets this device into, asked of its hub without using it;
/// `member`, the device's key, makes one that came in by it already told
/// what using it again would tell it. A hub from before managed machines
/// refuses the asking connection; that is said as `Ok(None)`, as such a hub
/// makes no token for one.
pub async fn peek(token: &Token, member: Option<&PublicKey>) -> Result<Option<Peeked>, String> {
    let dialed = dial::dial(&token.addresses, Some(token.cert), wire::CONTROL_PATH, wire::MAX_CONTROL_FRAME).await?;
    let (mut socket, binding) = (dialed.socket, dialed.binding);
    let key = token.secret.key();
    let intent =
        Intent::Peek { token: key.public(), proof: key.sign(Domain::Session, &binding), member: member.copied() };
    hello(&mut socket, intent).await?;
    let answer = next(&mut socket).await;
    let _ = futures_util::SinkExt::close(&mut socket).await;
    match answer {
        Ok(Down::TokenInfo { hub, hub_name, account, managed, label, expires }) => {
            if hub != token.hub {
                return Err(format!("the hub is {hub}, not {} the token names", token.hub));
            }
            Ok(Some(Peeked { hub, hub_name, account, managed, label, expires }))
        }
        Ok(Down::Refused { reason: crate::wire::Refusal::Login { reason } }) => Err(reason),
        Ok(Down::Refused { .. }) | Err(_) => Ok(None),
        Ok(_) => Err("the hub answered with something else than what the token is for".to_string()),
    }
}

/// Opens `account` with `password` on a hub that lets anyone, reached at
/// `hub`. The device then logs in to it like any other.
pub async fn open_account(hub: &[SocketAddr], account: &AccountName, password: &str) -> Result<(), String> {
    fastctx_types::password::check(account.as_str(), password)?;
    let dialed = dial::dial(hub, None, wire::CONTROL_PATH, wire::MAX_CONTROL_FRAME).await?;
    let mut socket = dialed.socket;
    let (state, request) = password_start(password)?;
    hello(&mut socket, Intent::Register { account: account.clone(), request }).await?;
    let response = match next(&mut socket).await? {
        Down::RegisterResponse { response } => response,
        Down::Refused { reason } => return Err(reason.to_string()),
        _ => return Err("the hub answered with something else than its half of the account".to_string()),
    };
    let upload = password_finish(state, password, &response)?;
    dial::send(&mut socket, &Up::RegisterFinish { upload }).await?;
    match next(&mut socket).await? {
        Down::Registered => Ok(()),
        Down::Refused { reason } => Err(reason.to_string()),
        _ => Err("the hub answered with something else than that the account is open".to_string()),
    }
}

/// A password being set, between its two messages.
pub struct PasswordState(ClientRegistration<Suite>);

/// The first message of setting a password.
pub fn password_start(password: &str) -> Result<(PasswordState, Bytes), String> {
    let start = ClientRegistration::<Suite>::start(&mut OsRng, password.as_bytes())
        .map_err(|error| format!("starting to set the password: {error}"))?;
    Ok((PasswordState(start.state), Bytes(start.message.serialize().to_vec())))
}

/// The last message of setting a password, from the hub's half.
pub fn password_finish(state: PasswordState, password: &str, response: &Bytes) -> Result<Bytes, String> {
    let response = RegistrationResponse::<Suite>::deserialize(&response.0)
        .map_err(|error| format!("the hub's half of the password: {error}"))?;
    let finished = state
        .0
        .finish(&mut OsRng, password.as_bytes(), response, ClientRegistrationFinishParameters::default())
        .map_err(|error| format!("setting the password: {error}"))?;
    Ok(Bytes(finished.message.serialize().to_vec()))
}

async fn hello(socket: &mut HubSocket, intent: Intent) -> Result<(), String> {
    let hello = Up::Hello { protocol: wire::GREETING_PROTOCOL, version: env!("CARGO_PKG_VERSION").to_string(), intent };
    dial::send(socket, &hello).await
}

async fn next(socket: &mut HubSocket) -> Result<Down, String> {
    match tokio::time::timeout(SILENCE, dial::recv(socket)).await {
        Ok(Ok(Some(frame))) => Ok(frame),
        Ok(Ok(None)) => Err("the hub closed the connection".to_string()),
        Ok(Err(error)) => Err(error),
        Err(_) => Err(format!("the hub said nothing for {} s", SILENCE.as_secs())),
    }
}

async fn joined(socket: &mut HubSocket, addresses: &[SocketAddr], cert: CertPin) -> Result<Joined, String> {
    let answer = next(socket).await;
    let _ = futures_util::SinkExt::close(socket).await;
    match answer? {
        Down::LoggedIn { hub, hub_name, name, account } => {
            Ok(Joined { hub, hub_name, name, account, addresses: addresses.to_vec(), cert })
        }
        Down::Refused { reason } => Err(reason.to_string()),
        _ => Err("the hub answered the login with something else".to_string()),
    }
}

/// The hub's lasting OPAQUE secrets: its key-exchange key and the seed of
/// every account's OPRF key. Kept private on the hub's disk, never changed.
pub struct HubSecrets(ServerSetup<Suite>);

impl HubSecrets {
    pub fn generate() -> Self {
        Self(ServerSetup::<Suite>::new(&mut OsRng))
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        self.0.serialize().to_vec()
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        ServerSetup::<Suite>::deserialize(bytes).map(Self).map_err(|error| format!("the hub's login secrets: {error}"))
    }

    /// The hub's half of a login to `account`, whose registration left
    /// `record`; an account that does not exist is answered alike, from a
    /// made-up record, so the answer does not tell whether it exists.
    /// Returns the answer for the device and what the hub keeps until the
    /// device finishes.
    pub fn login_start(
        &self,
        account: &AccountName,
        record: Option<&[u8]>,
        request: &Bytes,
        binding: &[u8; 32],
    ) -> Result<(Bytes, Vec<u8>), String> {
        let record = record
            .map(ServerRegistration::<Suite>::deserialize)
            .transpose()
            .map_err(|error| format!("the account's password record: {error}"))?;
        let request = CredentialRequest::<Suite>::deserialize(&request.0)
            .map_err(|error| format!("the login request: {error}"))?;
        let parameters = ServerLoginStartParameters { context: Some(binding), identifiers: Default::default() };
        let started = ServerLogin::start(&mut OsRng, &self.0, record, request, account.as_bytes(), parameters)
            .map_err(|error| format!("answering the login: {error}"))?;
        Ok((Bytes(started.message.serialize().to_vec()), started.state.serialize().to_vec()))
    }

    /// Whether the device's last message finishes the login `state` began:
    /// it does only when the device knows the password and is on the same
    /// TLS connection.
    pub fn login_finish(state: &[u8], finalization: &Bytes) -> Result<(), String> {
        let state = ServerLogin::<Suite>::deserialize(state).map_err(|error| format!("the login: {error}"))?;
        let finalization = CredentialFinalization::<Suite>::deserialize(&finalization.0)
            .map_err(|error| format!("the login's last message: {error}"))?;
        state.finish(finalization).map(|_| ()).map_err(|_| "the password is wrong".to_string())
    }

    /// The hub's half of setting the password of `account`.
    pub fn register_start(&self, account: &AccountName, request: &Bytes) -> Result<Bytes, String> {
        let request = RegistrationRequest::<Suite>::deserialize(&request.0)
            .map_err(|error| format!("the password request: {error}"))?;
        let started = ServerRegistration::<Suite>::start(&self.0, request, account.as_bytes())
            .map_err(|error| format!("answering the password request: {error}"))?;
        Ok(Bytes(started.message.serialize().to_vec()))
    }

    /// The record setting a password leaves with the hub, from the
    /// device's last message.
    pub fn register_finish(upload: &Bytes) -> Result<Vec<u8>, String> {
        let upload = RegistrationUpload::<Suite>::deserialize(&upload.0)
            .map_err(|error| format!("the password's last message: {error}"))?;
        Ok(ServerRegistration::<Suite>::finish(upload).serialize().to_vec())
    }
}
