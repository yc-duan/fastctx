//! Registration preserves birth ownership while updating a conversation's running address.

use super::*;
use fastctx_types::ResidentId;
use fastctx_types::access::GrantId;

/// What a host says of a conversation's birth as it registers it.
pub(super) struct Birth {
    /// Its original birth time, from which the canonical identity is mapped.
    pub at: Option<jiff::Timestamp>,
    /// The assistance it was born working by, for the account registering it.
    pub under: Option<GrantId>,
    /// When that account was opened, as it was born.
    pub generation: Option<jiff::Timestamp>,
    pub colleague: bool,
}

impl Birth {
    /// Keeps on `kept`, once, what of the birth holds: an account opened when
    /// it says, the account now of its name, and an assistance this hub gives
    /// that account of a login on `born_on`, the machine it was born on. What
    /// does not hold is let go, as the hub's log says (`dropped`). Whether it
    /// changed.
    fn keep(
        &self,
        hub: &Hub,
        directory: &fastctx_net::directory::Directory,
        born_on: &MachineName,
        kept: &mut Conversation,
    ) -> bool {
        let (changed, dropped) = self.kept(directory, born_on, kept);
        if let Some(dropped) = dropped {
            hub.log.line(format_args!("{dropped}"));
        }
        changed
    }

    fn kept(
        &self,
        directory: &fastctx_net::directory::Directory,
        born_on: &MachineName,
        kept: &mut Conversation,
    ) -> (bool, Option<String>) {
        if kept.born_under.is_some() || kept.generation.is_some() || kept.colleague {
            return (false, None);
        }
        let account = &kept.account;
        let generation =
            self.generation.filter(|generation| directory.account(account).is_some_and(|it| it.opened == *generation));
        let under = self.under.clone().filter(|grant| {
            directory.assists.iter().any(|assist| {
                assist.account == *account && directory.same(&assist.machine, born_on) && assist.grant_id() == *grant
            })
        });
        let dropped = (generation != self.generation || under != self.under).then(|| {
            format!(
                "{} said {} was born for {account} opened {:?} under {:?}; not so here, kept as {generation:?} under {under:?}",
                kept.address.machine, kept.address.resident, self.generation, self.under
            )
        });
        kept.born_under = under;
        kept.generation = generation;
        kept.colleague = self.colleague;
        (kept.born_under.is_some() || kept.generation.is_some() || kept.colleague, dropped)
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn register(
    hub: &Arc<Hub>,
    from: &MachineName,
    key: &PublicKey,
    account: &AccountName,
    resident: ResidentId,
    shared: bool,
    previous: Option<MachineName>,
    birth: Birth,
) -> Result<Conversation, String> {
    let born_at = birth.at;
    let name = resident.to_string();
    let read = RecordRequest::Read { kind: fastctx_types::owned_records::KIND.into(), key: name.clone() };
    let opened = account_records::begin(hub, from, key, account, &read)?;
    loop {
        let Told::AccountRecords { response: RecordResponse::Read { record } } =
            account_records::ask(hub, from.clone(), *key, account.clone(), opened, read.clone()).await?
        else {
            return Err("conversation registration received an unexpected reply".into());
        };
        let expected = record.as_ref().map(|record| record.metadata.version);
        // Mapping the canonical identity changes nothing a catalog shows.
        let mut quiet = false;
        let conversation = if let Some(record) = record {
            if record.metadata.deleted {
                return Err("this conversation was deleted".into());
            }
            let mut kept: Conversation = serde_json::from_value(record.body).map_err(|error| error.to_string())?;
            let store = hub.store();
            let directory = &store.book.directory;
            if directory.same(&kept.address.machine, from) {
                // Its host registers it again: the canonical identity is mapped
                // once, from the birth machine's key, when it is still missing,
                // and what it was born as is kept once, by its birth machine.
                let born = kept.previous.first().map_or(Some(*key), |machine| directory.member(machine).map(|m| m.key));
                let born_on = kept.previous.first().unwrap_or(&kept.address.machine).clone();
                let mapped = match (kept.identity.is_none(), born, born_at) {
                    (true, Some(device), Some(born_at)) => {
                        kept.identity = Some(Box::new(identity(&store.hub.id, &device, &resident, born_at)));
                        true
                    }
                    _ => false,
                };
                let as_born = birth.keep(hub, directory, &born_on, &mut kept);
                if !mapped && !as_born {
                    return Ok(kept);
                }
                quiet = true;
            } else {
                if previous.as_ref().is_none_or(|source| {
                    !directory.same(source, &kept.address.machine)
                        && !kept.previous.iter().any(|old| directory.same(source, old))
                }) {
                    return Err("the conversation's previous address does not match its registered owner".into());
                }
                if !kept.previous.contains(&kept.address.machine) {
                    kept.previous.push(kept.address.machine.clone());
                }
                kept.address.machine = from.clone();
            }
            kept
        } else {
            let source = previous.clone().unwrap_or_else(|| from.clone());
            let store = hub.store();
            let born = shared
                .then(|| {
                    let member = store.book.directory.member(&source)?;
                    Some((member.name.clone(), fastctx_net::record_ownership::seal(member)?))
                })
                .flatten();
            let device = if source == *from { Some(*key) } else { store.book.directory.member(&source).map(|m| m.key) };
            let mut conversation = Conversation {
                account: account.clone(),
                address: Address { machine: from.clone(), resident: resident.clone() },
                shared_on: born.as_ref().map(|(machine, _)| machine.clone()),
                born_seal: born.map(|(_, key)| key),
                previous: if source != *from { vec![source.clone()] } else { Vec::new() },
                identity: device
                    .zip(born_at)
                    .map(|(device, born_at)| Box::new(identity(&store.hub.id, &device, &resident, born_at))),
                born_under: None,
                generation: None,
                colleague: false,
            };
            birth.keep(hub, &store.book.directory, &source, &mut conversation);
            conversation
        };
        let request = RecordRequest::Write {
            kind: fastctx_types::owned_records::KIND.into(),
            key: name.clone(),
            expected,
            body: serde_json::to_value(&conversation).map_err(|error| error.to_string())?,
        };
        match account_records::ask(hub, from.clone(), *key, account.clone(), opened, request).await? {
            Told::AccountRecords { response: RecordResponse::Written { .. } } => {
                if !quiet {
                    super::notify(hub, &conversation);
                }
                return Ok(conversation);
            }
            Told::AccountRecords { response: RecordResponse::Conflict { .. } } => {}
            _ => return Err("conversation registration received an unexpected reply".into()),
        }
    }
}

/// The canonical conversation a host's birth facts name: this hub, the birth
/// machine's device key, the resident and its original birth time.
fn identity(
    hub: &fastctx_types::access::HubId,
    device: &PublicKey,
    resident: &ResidentId,
    born_at: jiff::Timestamp,
) -> fastctx_types::sync::identity::ObjectId {
    fastctx_types::sync::conversation_identity::ConversationKey::Birth {
        hub: hub.clone(),
        device: fastctx_types::sync::identity::DeviceId::from_bytes(*device.as_bytes()),
        resident: resident.clone(),
        born_at,
    }
    .id()
}

#[cfg(test)]
mod tests {
    use super::*;
    use fastctx_net::directory::Directory;

    const OPENED: &str = "2026-11-01T00:00:00Z";

    fn directory() -> Directory {
        serde_json::from_value(serde_json::json!({
            "accounts": [{"name": "carol", "level": "member", "locked": false, "opened": OPENED}],
            "members": [],
            "projects": [],
            "lends": [
                {"machine": "host", "account": "carol", "agent": "codex", "grant": "g-00000000000000aa"},
                {"machine": "host", "account": "carol", "agent": "claude"},
                {"machine": "other", "account": "carol", "agent": "codex", "grant": "g-00000000000000bb"},
            ],
        }))
        .expect("a directory")
    }

    fn conversation() -> Conversation {
        Conversation {
            account: "carol".parse().expect("an account"),
            address: Address {
                machine: "host".parse().expect("a machine"),
                resident: "codex-ab12".parse().expect("a resident"),
            },
            shared_on: None,
            born_seal: None,
            previous: Vec::new(),
            identity: None,
            born_under: None,
            generation: None,
            colleague: false,
        }
    }

    fn birth(under: Option<&str>, generation: Option<&str>) -> Birth {
        Birth {
            at: None,
            under: under.map(|grant| GrantId::try_from(grant.to_string()).expect("a grant")),
            generation: generation.map(|at| at.parse().expect("a time")),
            colleague: false,
        }
    }

    #[test]
    fn what_a_conversation_was_born_as_is_kept_once_where_it_holds() {
        let directory = directory();
        let host: MachineName = "host".parse().expect("a machine");
        // As its host says it, where it holds.
        let mut kept = conversation();
        let (changed, dropped) = birth(Some("g-00000000000000aa"), Some(OPENED)).kept(&directory, &host, &mut kept);
        assert!(changed && dropped.is_none());
        assert_eq!(kept.born_under.as_ref().map(GrantId::as_str), Some("g-00000000000000aa"));
        assert_eq!(kept.generation, Some(OPENED.parse().expect("a time")));
        // Never changed after.
        let (changed, _) = birth(None, Some("2026-11-02T00:00:00Z")).kept(&directory, &host, &mut kept);
        assert!(!changed);
        assert_eq!(kept.born_under.as_ref().map(GrantId::as_str), Some("g-00000000000000aa"));
        // An assistance kept before grants were is known by the grant read from it.
        let legacy = directory.assists[1].grant_id();
        let mut kept = conversation();
        birth(Some(legacy.as_str()), Some(OPENED)).kept(&directory, &host, &mut kept);
        assert_eq!(kept.born_under, Some(legacy));
        // Another account of the name, another machine's assistance, or one
        // the hub never gave, is let go and said.
        for (under, generation) in [
            (Some("g-00000000000000aa"), Some("2026-11-02T00:00:00Z")),
            (Some("g-00000000000000bb"), Some(OPENED)),
            (Some("g-00000000000000cc"), Some(OPENED)),
        ] {
            let mut kept = conversation();
            let (_, dropped) = birth(under, generation).kept(&directory, &host, &mut kept);
            assert!(dropped.is_some());
            assert!(kept.born_under.is_none() || kept.generation.is_none());
        }
    }
}
