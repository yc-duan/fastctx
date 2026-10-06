//! Joining a World with a code: the hub's founding code for the first
//! machine, an invitation for every one after it.

use fastctx_types::MachineName;

use crate::code::Code;
use crate::dial;
use crate::link::{Keys, Membership};
use crate::sign::{Domain, SecretKey, Signed};
use crate::trust::{Admission, Credential, Record};
use crate::wire::{self, Down, Intent, PROTOCOL, SILENCE, Up};

/// Joins the World `code` is for as `name`, with this machine's `keys`.
pub async fn join(code: &Code, name: MachineName, keys: &Keys) -> Result<Membership, String> {
    attempt(code, name, keys).await.map_err(|error| error.to_string())
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
pub async fn attempt(code: &Code, name: MachineName, keys: &Keys) -> Result<Membership, JoinError> {
    let key = &keys.sign;
    let (credential, founding, founder) = match code {
        Code::Founding { secret, .. } => (Credential::Founder, Some(secret.clone()), key.public()),
        Code::Invitation { endorsement, seed, founder, .. } => {
            let invite = SecretKey::from_seed(&seed.0);
            let admission = Admission { endorsement: endorsement.clone(), name: name.clone(), key: key.public() };
            let admission = Signed::new(&invite, Domain::Admission, &admission);
            (Credential::Invited { admission }, None, *founder)
        }
    };
    let record = Record {
        world: code.world().clone(),
        name: name.clone(),
        key: key.public(),
        credential,
        seal: Some(keys.seal.public()),
    };
    let record = Signed::new(key, Domain::Record, &record);
    let dialed = dial::dial(code.hub(), code.cert(), wire::CONTROL_PATH, wire::MAX_CONTROL_FRAME)
        .await
        .map_err(JoinError::NotSent)?;
    let mut socket = dialed.socket;
    let hello = Up::Hello {
        protocol: PROTOCOL,
        version: env!("CARGO_PKG_VERSION").to_string(),
        intent: Intent::Join { record, founding },
    };
    dial::send(&mut socket, &hello).await?;
    let answer = tokio::time::timeout(SILENCE, dial::recv(&mut socket))
        .await
        .map_err(|_| format!("the hub did not answer within {} s", SILENCE.as_secs()))??;
    match answer {
        Some(Down::Joined { world, name: joined }) if world == *code.world() && joined == name => {
            Ok(Membership { world, name, founder, hub: code.hub().to_vec(), cert: code.cert() })
        }
        Some(Down::Joined { world, name }) => {
            Err(JoinError::Uncertain(format!("the hub admitted {name} into {world}, which is not what was asked")))
        }
        Some(Down::Refused { reason }) => Err(JoinError::Refused(reason.to_string())),
        Some(_) => Err(JoinError::Uncertain("the hub answered something other than an admission".to_string())),
        None => Err(JoinError::Uncertain("the hub closed the connection without answering".to_string())),
    }
}
