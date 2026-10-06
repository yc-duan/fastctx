//! Admitting a machine: the founder with the founding secret, everyone
//! after with an invitation a member made.
//!
//! Admission happens under the store's lock, so one invitation, one name or
//! the founding secret can never admit two machines racing for it. The same
//! machine admitted again, with the same name, key and invitation, is told
//! it joined: the reply to its admission may never have reached it.

use fastctx_net::sign::{Domain, PublicKey, Secret, Signature, Signed};
use fastctx_net::trust::{self, Admission, Credential, Endorsement, Known, Record, WorldId};
use fastctx_types::MachineName;
use jiff::Timestamp;

use crate::Hub;
use crate::store::{Admitted, Store};

/// The invitation whose key `invite` is, for a machine that proved on this
/// connection that it holds that key: one not used yet, not expired, made by
/// a member still trusted; or one used already, which only the machine it
/// admitted can be admitted with again.
pub(crate) fn endorsement(
    hub: &Hub,
    invite: &PublicKey,
    proof: &Signature,
    binding: &[u8; 32],
) -> Result<Signed, String> {
    invite.verify(Domain::Session, binding, proof).map_err(|_| "the proof of the invitation's key does not verify")?;
    let store = hub.store();
    let invitation = store.invitation(invite).cloned().ok_or("the invitation is not known to the hub")?;
    if invitation.used_by.is_none() {
        usable(&store, &invitation)?;
    }
    Ok(invitation.endorsement)
}

fn usable(store: &Store, invitation: &crate::store::Invitation) -> Result<(), String> {
    if let Some(used_by) = &invitation.used_by {
        return Err(format!("the invitation was used already, by {used_by}"));
    }
    if invitation.expires <= Timestamp::now() {
        return Err(format!("the invitation expired at {}", invitation.expires.strftime("%Y-%m-%d %H:%M:%S UTC")));
    }
    if store.table().get(&invitation.inviter).is_none() {
        return Err(format!("{}, who made the invitation, is no longer a member", invitation.inviter));
    }
    Ok(())
}

/// A machine joined: the World, its name, and whether it is new, rather than
/// told again that it joined.
pub(crate) struct Joined {
    pub world: WorldId,
    pub name: MachineName,
    pub new: bool,
}

pub(crate) fn join(hub: &Hub, signed: &Signed, founding: Option<&Secret>) -> Result<Joined, String> {
    let claimed: Record = signed.claim().map_err(|error| format!("the record: {error}"))?;
    let record: Record = signed.open(&claimed.key, Domain::Record).map_err(|error| format!("the record: {error}"))?;
    let mut store = hub.store();
    let world = store.hub.world.clone();
    if record.world != world {
        return Err(format!("the code is for the World {}, and this hub keeps {world}", record.world));
    }
    if let Some(existing) = store.members.records.iter().find(|admitted| admitted.name == record.name) {
        let existing: Record = existing.record.claim().map_err(|error| format!("the record the hub keeps: {error}"))?;
        if existing.key != record.key {
            return Err(format!("the World has a machine named {} already; join with another name", record.name));
        }
        if invite_of(&existing) != invite_of(&record) {
            return Err(format!("this machine is a member already, as {}", record.name));
        }
        if store.table().get(&record.name).is_none_or(|member| member.key != record.key) {
            return Err(format!("{} was a member, and is not trusted any more", record.name));
        }
        // The founder proves itself with the secret it founded with; an
        // invited machine proved it holds the invitation's key before this.
        if matches!(record.credential, Credential::Founder) {
            let used = founding.map(|secret| fastctx_net::sign::Digest(fastctx_net::sign::sha256(&*secret.0)));
            if used.is_none() || used != store.hub.founded_with {
                return Err("the founding code was used already, or is not this hub's".to_string());
            }
        }
        return Ok(Joined { world, name: record.name, new: false });
    }
    if let Some(member) = store.table().by_key(&record.key) {
        return Err(format!("this machine's key is a member already, as {}", member.name));
    }

    match (&record.credential, founding) {
        (Credential::Founder, Some(secret)) => {
            if !store.members.records.is_empty() || store.hub.founding.as_ref() != Some(secret) {
                return Err("the founding code was used already, or is not this hub's".to_string());
            }
            admit(&mut store, &record, signed)?;
            store.hub.founded_with = Some(fastctx_net::sign::Digest(fastctx_net::sign::sha256(&*secret.0)));
            store.hub.founding = None;
            store.save_hub().map_err(|error| format!("the hub could not record the founding: {error}"))?;
        }
        (Credential::Invited { admission }, None) => {
            let admission: Admission = admission.claim().map_err(|error| format!("the admission: {error}"))?;
            let endorsement: Endorsement =
                admission.endorsement.claim().map_err(|error| format!("the endorsement: {error}"))?;
            let invitation =
                store.invitation(&endorsement.invite).cloned().ok_or("the invitation is not known to the hub")?;
            usable(&store, &invitation)?;
            // The new record must check out the way every machine will
            // check it.
            let founder = store.founder().ok_or("the World has no founder yet")?;
            let mut records: Vec<Signed> =
                store.members.records.iter().map(|admitted| admitted.record.clone()).collect();
            records.push(signed.clone());
            let (table, _) = trust::check(
                &world,
                &founder,
                &records,
                &store.members.revocations,
                &store.members.grants,
                &Known::default(),
            );
            if table.get(&record.name).is_none() {
                let reason = table
                    .refused
                    .iter()
                    .find(|(name, _)| name == record.name.as_str())
                    .map(|(_, reason)| reason.clone())
                    .unwrap_or_else(|| "it does not check out".to_string());
                return Err(format!("the machine's record: {reason}"));
            }
            admit(&mut store, &record, signed)?;
            if let Some(invitation) =
                store.members.invitations.iter_mut().find(|invitation| invitation.invite == endorsement.invite)
            {
                invitation.used_by = Some(record.name.clone());
            }
            store.save_members().map_err(|error| format!("the hub could not record the invitation's use: {error}"))?;
        }
        (Credential::Founder, None) => {
            return Err("a founder joins with the founding code".to_string());
        }
        (Credential::Invited { .. }, Some(_)) => {
            return Err("an invited machine joins with its invitation".to_string());
        }
    }
    Ok(Joined { world, name: record.name, new: true })
}

/// The key of the invitation that admits `record`; `None` for the founder.
fn invite_of(record: &Record) -> Option<PublicKey> {
    match &record.credential {
        Credential::Founder => None,
        Credential::Invited { admission } => admission
            .claim::<Admission>()
            .ok()
            .and_then(|admission| admission.endorsement.claim::<Endorsement>().ok())
            .map(|endorsement| endorsement.invite),
    }
}

fn admit(store: &mut Store, record: &Record, signed: &Signed) -> Result<(), String> {
    let letters = Store::new_letters().map_err(|error| error.to_string())?;
    store.letters.insert(record.name.clone(), letters);
    store.save_letters().map_err(|error| format!("the hub could not record the new member: {error}"))?;
    store.members.records.push(Admitted {
        name: record.name.clone(),
        record: signed.clone(),
        joined_at: Timestamp::now(),
    });
    store.save_members().map_err(|error| format!("the hub could not record the new member: {error}"))
}
