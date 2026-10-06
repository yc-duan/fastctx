//! Joining a World with a code: the hub's founding code for the first
//! machine, an invitation for every one after it.
//!
//! With an invitation the joining machine first proves it holds the
//! invitation's key and gets the invitation from the hub; it checks what the
//! invitation admits before it admits anything, so a code meant for a phone
//! is not used up by a machine, and only then sends its own record.

use fastctx_types::MachineName;

use crate::code::Code;
use crate::dial::{self, HubSocket};
use crate::link::{Keys, Membership};
use crate::sign::{Domain, Signed};
use crate::trust::{Admission, Credential, Endorsement, Kind, Record};
use crate::wire::{self, Down, Intent, PROTOCOL, SILENCE, Up};

/// What a joining member asks of its invitation.
#[derive(Clone, Debug, Default)]
pub struct Asking {
    /// The name it takes; when not given, the one the invitation fixes, and
    /// then `fallback`.
    pub name: Option<MachineName>,
    pub fallback: Option<MachineName>,
    /// What it must admit: a machine joins with a machine's invitation, a
    /// device with a device's.
    pub kind: Kind,
}

/// Joins the World `code` is for, with this machine's `keys`.
pub async fn join(code: &Code, asking: Asking, keys: &Keys) -> Result<Membership, String> {
    attempt(code, asking, keys, |_| Ok(())).await.map_err(|error| error.to_string())
}

/// A refusal is authoritative. A lost response leaves admission uncertain:
/// keep the key and prospective membership so the next session can recover.
#[derive(Debug, thiserror::Error)]
pub enum JoinError {
    #[error("{0}; no admission was sent")]
    NotSent(String),
    #[error("{0}")]
    Refused(String),
    #[error("admission was not confirmed: {0}")]
    Uncertain(String),
}

impl From<String> for JoinError {
    fn from(reason: String) -> Self {
        Self::Uncertain(reason)
    }
}

/// As [`join`], distinguishing refusal from an admission whose reply was lost.
/// `before` is told the name the member takes just before its record goes
/// to the hub: from then on admission may happen without its reply arriving.
pub async fn attempt(
    code: &Code,
    asking: Asking,
    keys: &Keys,
    before: impl FnOnce(&MachineName) -> Result<(), String>,
) -> Result<Membership, JoinError> {
    let key = &keys.sign;
    let dialed = dial::dial(code.hub(), code.cert(), wire::CONTROL_PATH, wire::MAX_CONTROL_FRAME)
        .await
        .map_err(JoinError::NotSent)?;
    let mut socket = dialed.socket;
    let hello = |intent| Up::Hello { protocol: PROTOCOL, version: env!("CARGO_PKG_VERSION").to_string(), intent };
    let record = |name: &MachineName, credential| {
        let record = Record {
            world: code.world().clone(),
            name: name.clone(),
            key: key.public(),
            credential,
            seal: Some(keys.seal.public()),
            relay: None,
        };
        Signed::new(key, Domain::Record, &record)
    };
    let (name, founder) = match code {
        Code::Founding { secret, .. } => {
            if asking.kind != Kind::Machine {
                return Err(JoinError::NotSent("a World is founded by a machine".to_string()));
            }
            let name = asking.name.or(asking.fallback).ok_or_else(|| JoinError::NotSent("give a name".to_string()))?;
            before(&name).map_err(JoinError::NotSent)?;
            let intent = Intent::Join { record: record(&name, Credential::Founder), founding: Some(secret.clone()) };
            dial::send(&mut socket, &hello(intent)).await.map_err(JoinError::NotSent)?;
            (name, key.public())
        }
        Code::Invitation { secret, founder, .. } => {
            let invite = secret.key();
            let intent =
                Intent::Invited { invite: invite.public(), proof: invite.sign(Domain::Session, &dialed.binding) };
            dial::send(&mut socket, &hello(intent)).await.map_err(JoinError::NotSent)?;
            let endorsement = match answer(&mut socket).await.map_err(JoinError::NotSent)? {
                Down::Endorsement { endorsement } => endorsement,
                Down::Refused { reason } => return Err(JoinError::Refused(reason.to_string())),
                _ => {
                    return Err(JoinError::NotSent("the hub answered something other than the invitation".to_string()));
                }
            };
            let claimed: Endorsement = endorsement
                .claim()
                .map_err(|error| JoinError::NotSent(format!("the invitation cannot be read: {error}")))?;
            if claimed.invite != invite.public() || claimed.world != *code.world() {
                return Err(JoinError::NotSent("the hub gave an invitation that is not this code's".to_string()));
            }
            if claimed.kind != asking.kind {
                return Err(JoinError::NotSent(
                    match claimed.kind {
                        Kind::Device => "this invitation admits a device, such as a phone; ask for one for a machine",
                        Kind::Machine => "this invitation admits a machine; ask for a device invitation",
                    }
                    .to_string(),
                ));
            }
            if let (Some(fixed), Some(given)) = (&claimed.name, &asking.name)
                && fixed != given
            {
                return Err(JoinError::NotSent(format!("this invitation admits a member named {fixed}, not {given}")));
            }
            let name = asking
                .name
                .or(claimed.name)
                .or(asking.fallback)
                .ok_or_else(|| JoinError::NotSent("give this member a name".to_string()))?;
            before(&name).map_err(JoinError::NotSent)?;
            let admission = Admission { endorsement, name: name.clone(), key: key.public() };
            let admission = Signed::new(&invite, Domain::Admission, &admission);
            dial::send(&mut socket, &Up::Admit { record: record(&name, Credential::Invited { admission }) }).await?;
            (name, *founder)
        }
    };
    match answer(&mut socket).await? {
        Down::Joined { world, name: joined } if world == *code.world() && joined == name => {
            Ok(Membership { world, name, founder, hub: code.hub().to_vec(), cert: code.cert() })
        }
        Down::Joined { world, name } => {
            Err(JoinError::Uncertain(format!("the hub admitted {name} into {world}, which is not what was asked")))
        }
        Down::Refused { reason } => Err(JoinError::Refused(reason.to_string())),
        _ => Err(JoinError::Uncertain("the hub answered something other than an admission".to_string())),
    }
}

/// The hub's next frame, within the time it gives any silent peer.
async fn answer(socket: &mut HubSocket) -> Result<Down, String> {
    match tokio::time::timeout(SILENCE, dial::recv(socket)).await {
        Ok(Ok(Some(frame))) => Ok(frame),
        Ok(Ok(None)) => Err("the hub closed the connection without answering".to_string()),
        Ok(Err(error)) => Err(error),
        Err(_) => Err(format!("the hub did not answer within {} s", SILENCE.as_secs())),
    }
}
