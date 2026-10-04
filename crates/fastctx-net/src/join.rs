//! Joining a World with a code: the hub's founding code for the first
//! machine, an invitation for every one after it.

use fastctx_types::MachineName;

use crate::code::Code;
use crate::dial;
use crate::link::Membership;
use crate::sign::{Domain, SecretKey, Signed};
use crate::trust::{Admission, Credential, Record};
use crate::wire::{self, Down, Intent, PROTOCOL, SILENCE, Up};

/// Joins the World `code` is for as `name`, with this machine's `key`.
pub async fn join(code: &Code, name: MachineName, key: &SecretKey) -> Result<Membership, String> {
    let (credential, founding, founder) = match code {
        Code::Founding { secret, .. } => (Credential::Founder, Some(secret.clone()), key.public()),
        Code::Invitation { endorsement, seed, founder, .. } => {
            let invite = SecretKey::from_seed(&seed.0);
            let admission = Admission { endorsement: endorsement.clone(), name: name.clone(), key: key.public() };
            let admission = Signed::new(&invite, Domain::Admission, &admission);
            (Credential::Invited { admission }, None, *founder)
        }
    };
    let record = Record { world: code.world().clone(), name: name.clone(), key: key.public(), credential };
    let record = Signed::new(key, Domain::Record, &record);
    let dialed = dial::dial(code.hub(), code.cert(), wire::CONTROL_PATH, wire::MAX_CONTROL_FRAME).await?;
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
            Err(format!("the hub admitted {name} into {world}, which is not what was asked"))
        }
        Some(Down::Refused { reason }) => Err(reason.to_string()),
        Some(_) => Err("the hub answered something other than an admission".to_string()),
        None => Err("the hub closed the connection without answering".to_string()),
    }
}
