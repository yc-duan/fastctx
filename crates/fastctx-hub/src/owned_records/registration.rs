//! Registration preserves birth ownership while updating a conversation's running address.

use super::*;
use fastctx_net::directory::Directory;
use fastctx_types::AccountName;
use fastctx_types::ResidentId;
use fastctx_types::access::GrantId;
use fastctx_types::owned_records::Birth as Born;

/// What a host says of a conversation's birth as it registers it.
pub(super) struct Birth {
    /// Its original birth time, from which the canonical identity is mapped.
    pub at: Option<jiff::Timestamp>,
    /// When the account registering it was opened, as it was born; none
    /// where the host says nothing of its birth, as before it was born whole.
    pub generation: Option<jiff::Timestamp>,
    /// It was born working by an assistance of the login.
    pub assisted: bool,
    /// That assistance, as the host's hub gave it.
    pub under: Option<GrantId>,
    pub colleague: bool,
}

/// Whether the hub gives `account`, of a login on `born_on`, the assistance
/// `grant`.
fn gives(directory: &Directory, account: &AccountName, born_on: &MachineName, grant: &GrantId) -> bool {
    directory.assists.iter().any(|assist| {
        assist.account == *account && directory.same(&assist.machine, born_on) && assist.grant.as_ref() == Some(grant)
    })
}

impl Birth {
    /// `kept`, said in the hub's log; whether it changed.
    fn keep(
        &self,
        hub: &Hub,
        directory: &Directory,
        born_on: &MachineName,
        kept: &mut Conversation,
    ) -> Result<bool, String> {
        let (changed, said) = self.kept(directory, born_on, kept)?;
        if let Some(said) = said {
            hub.log.line(format_args!("{said}"));
        }
        Ok(changed)
    }

    /// Keeps on `kept` what its host says it was born as, once
    /// (design-forget-account-v2.md F0): which account of its name it was
    /// born for, whether by an assistance, and which, where the hub finds it
    /// gives that account of a login on `born_on`, the machine it was born
    /// on; one it does not find, as one ended before the conversation was
    /// registered, is kept as still to be found, never as none. Once kept,
    /// only the very assistance still to be found may be found; one said
    /// with none stays so. A birth said of an
    /// account of the name other than the one now is refused: the
    /// conversation is not this account's (codex-87c8bbd9). Whether it
    /// changed, and what the hub's log says.
    fn kept(
        &self,
        directory: &Directory,
        born_on: &MachineName,
        kept: &mut Conversation,
    ) -> Result<(bool, Option<String>), String> {
        let Some(generation) = self.generation else { return Ok((false, None)) };
        if directory.account(&kept.account).is_none_or(|account| account.opened != generation) {
            return Err(format!(
                "{} was born working for an account named {} that is not the one now",
                kept.address.resident, kept.account
            ));
        }
        let under = self.under.as_ref().filter(|_| self.assisted);
        if kept.birth.generation.is_some() {
            let found = kept.birth.assisted
                && kept.birth.born_under.is_none()
                && under.is_some_and(|grant| {
                    kept.birth.claimed_under.as_ref() == Some(grant) && gives(directory, &kept.account, born_on, grant)
                });
            if !found {
                return Ok((false, None));
            }
            kept.birth.born_under = under.cloned();
            kept.birth.claimed_under = None;
            return Ok((true, None));
        }
        kept.birth.generation = Some(generation);
        kept.birth.assisted = self.assisted;
        kept.birth.colleague = self.colleague;
        let mut said = None;
        if self.assisted {
            match under {
                Some(grant) if gives(directory, &kept.account, born_on, grant) => {
                    kept.birth.born_under = Some(grant.clone())
                }
                _ => {
                    kept.birth.claimed_under = under.cloned();
                    said = Some(format!(
                        "{} said {} was born working by {under:?}, an assistance the hub does not give {} on {born_on}: kept as still to be found",
                        kept.address.machine, kept.address.resident, kept.account
                    ));
                }
            }
        }
        Ok((true, said))
    }
}

/// The machine `conversation` was born on: the first it ran on.
fn born_on(conversation: &Conversation) -> &MachineName {
    conversation.previous.first().unwrap_or(&conversation.address.machine)
}

/// What a write of a conversation's ownership may do to what it was born as,
/// whichever way the write comes (codex-87c8bbd9): keep it once, whole and as
/// it holds (`Birth::kept`), and find an assistance still to be found;
/// nothing else.
pub(crate) fn birth_kept(directory: &Directory, old: Option<&Conversation>, new: &Conversation) -> Result<(), String> {
    // Where it was born, as kept: what a write adds to its addresses does not
    // move it (codex-87c8bbd9).
    let born_on = old.map_or_else(|| born_on(new), born_on);
    if old.is_some_and(|old| old.birth == new.birth) {
        return Ok(());
    }
    let rewritten = || Err("what a conversation was born as is kept once and never rewritten".to_string());
    if let Some(old) = old.filter(|old| old.birth.generation.is_some()) {
        let found = old.birth.assisted
            && new.birth.assisted
            && old.birth.born_under.is_none()
            && new.birth.claimed_under.is_none()
            && new.birth.generation == old.birth.generation
            && new.birth.colleague == old.birth.colleague
            && new.birth.born_under.as_ref().is_some_and(|grant| {
                old.birth.claimed_under.as_ref() == Some(grant) && gives(directory, &new.account, born_on, grant)
            });
        return if found { Ok(()) } else { rewritten() };
    }
    let Some(generation) = new.birth.generation else {
        return if *new.birth == Born::default() { Ok(()) } else { rewritten() };
    };
    if directory.account(&new.account).is_none_or(|account| account.opened != generation) {
        return Err("a conversation is kept for the account of its name it was born for".into());
    }
    let whole = (new.birth.assisted || (new.birth.born_under.is_none() && new.birth.claimed_under.is_none()))
        && !(new.birth.born_under.is_some() && new.birth.claimed_under.is_some())
        && new.birth.born_under.as_ref().is_none_or(|grant| gives(directory, &new.account, born_on, grant));
    if whole { Ok(()) } else { rewritten() }
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
                let born_on = born_on(&kept).clone();
                let mapped = match (kept.identity.is_none(), born, born_at) {
                    (true, Some(device), Some(born_at)) => {
                        kept.identity = Some(Box::new(identity(&store.hub.id, &device, &resident, born_at)));
                        true
                    }
                    _ => false,
                };
                let as_born = birth.keep(hub, directory, &born_on, &mut kept)?;
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
                // What it was born as, said again by its new host, is kept as by
                // its first, where it was born (codex-87c8bbd9).
                let born_on = born_on(&kept).clone();
                birth.keep(hub, directory, &born_on, &mut kept)?;
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
                birth: Default::default(),
            };
            birth.keep(hub, &store.book.directory, &source, &mut conversation)?;
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

    const OPENED: &str = "2026-11-01T00:00:00Z";
    const OTHER: &str = "2026-11-02T00:00:00Z";
    const GIVEN: &str = "g-00000000000000aa";

    fn directory() -> Directory {
        serde_json::from_value(serde_json::json!({
            "accounts": [{"name": "carol", "level": "member", "locked": false, "opened": OPENED}],
            "members": [],
            "projects": [],
            "lends": [
                {"machine": "host", "account": "carol", "agent": "codex", "grant": GIVEN},
                {"machine": "other", "account": "carol", "agent": "codex", "grant": "g-00000000000000bb"},
            ],
        }))
        .expect("a directory")
    }

    fn host() -> MachineName {
        "host".parse().expect("a machine")
    }

    fn conversation() -> Conversation {
        Conversation {
            account: "carol".parse().expect("an account"),
            address: Address { machine: host(), resident: "codex-ab12".parse().expect("a resident") },
            shared_on: None,
            born_seal: None,
            previous: Vec::new(),
            identity: None,
            birth: Default::default(),
        }
    }

    fn grant(text: &str) -> GrantId {
        GrantId::try_from(text.to_string()).expect("a grant")
    }

    fn birth(generation: Option<&str>, under: Option<&str>) -> Birth {
        Birth {
            at: None,
            generation: generation.map(|at| at.parse().expect("a time")),
            assisted: under.is_some(),
            under: under.map(grant),
            colleague: false,
        }
    }

    #[test]
    fn what_a_conversation_was_born_as_is_kept_once_as_it_holds() {
        let directory = directory();
        // Under an assistance the hub gives: found, kept, and a write may keep it.
        let mut kept = conversation();
        let (changed, said) = birth(Some(OPENED), Some(GIVEN)).kept(&directory, &host(), &mut kept).expect("kept");
        assert!(changed && said.is_none());
        assert_eq!(kept.birth.born_under, Some(grant(GIVEN)));
        assert!(kept.birth.assisted && kept.birth.claimed_under.is_none());
        assert!(birth_kept(&directory, Some(&conversation()), &kept).is_ok());
        // Never rewritten after, by registering again or by a write.
        let (changed, _) = birth(Some(OPENED), None).kept(&directory, &host(), &mut kept).expect("kept");
        assert!(!changed && kept.birth.born_under == Some(grant(GIVEN)));
        let mut cleared = kept.clone();
        cleared.birth.born_under = None;
        assert!(birth_kept(&directory, Some(&kept), &cleared).is_err());
        let mut forgotten = kept.clone();
        forgotten.birth.generation = None;
        assert!(birth_kept(&directory, Some(&kept), &forgotten).is_err());
        // Held by the machine: kept as such.
        let mut held = conversation();
        birth(Some(OPENED), None).kept(&directory, &host(), &mut held).expect("kept");
        assert!(held.birth.generation.is_some() && !held.birth.assisted && held.birth.born_under.is_none());
        // Nothing said: nothing kept.
        let mut unsaid = conversation();
        let (changed, _) = birth(None, None).kept(&directory, &host(), &mut unsaid).expect("kept");
        assert!(!changed && unsaid.birth.generation.is_none());
    }

    #[test]
    fn an_assistance_not_found_is_kept_still_to_be_found_and_found_once() {
        let directory = directory();
        // Of another machine, or one the hub never gave: still to be found, never none.
        for said in ["g-00000000000000bb", "g-00000000000000cc"] {
            let mut kept = conversation();
            let (changed, logged) = birth(Some(OPENED), Some(said)).kept(&directory, &host(), &mut kept).expect("kept");
            assert!(changed && logged.is_some());
            assert!(kept.birth.assisted && kept.birth.born_under.is_none());
            assert_eq!(kept.birth.claimed_under, Some(grant(said)));
            assert!(birth_kept(&directory, Some(&conversation()), &kept).is_ok());
        }
        // Found once the hub gives it, and only that one.
        let mut kept = conversation();
        kept.birth.generation = Some(OPENED.parse().expect("a time"));
        kept.birth.assisted = true;
        kept.birth.claimed_under = Some(grant(GIVEN));
        let before = kept.clone();
        let (changed, _) =
            birth(Some(OPENED), Some("g-00000000000000cc")).kept(&directory, &host(), &mut kept).expect("kept");
        assert!(!changed && kept.birth.born_under.is_none());
        let (changed, _) = birth(Some(OPENED), Some(GIVEN)).kept(&directory, &host(), &mut kept).expect("kept");
        assert!(changed && kept.birth.born_under == Some(grant(GIVEN)) && kept.birth.claimed_under.is_none());
        assert!(birth_kept(&directory, Some(&before), &kept).is_ok());
        // One said with none, as on a hub from before grants, stays still to
        // be found: no assistance given since is taken for it.
        let mut unnamed = conversation();
        unnamed.birth.generation = Some(OPENED.parse().expect("a time"));
        unnamed.birth.assisted = true;
        let (changed, _) = birth(Some(OPENED), Some(GIVEN)).kept(&directory, &host(), &mut unnamed).expect("kept");
        assert!(!changed && unnamed.birth.born_under.is_none());
        let mut taken = unnamed.clone();
        taken.birth.born_under = Some(grant(GIVEN));
        assert!(birth_kept(&directory, Some(&unnamed), &taken).is_err());
        // A write may not find one the hub does not give.
        let mut wrong = before.clone();
        wrong.birth.born_under = Some(grant("g-00000000000000cc"));
        wrong.birth.claimed_under = None;
        assert!(birth_kept(&directory, Some(&before), &wrong).is_err());
    }

    #[test]
    fn a_birth_said_of_another_account_of_the_name_is_refused() {
        let directory = directory();
        let mut kept = conversation();
        assert!(birth(Some(OTHER), Some(GIVEN)).kept(&directory, &host(), &mut kept).is_err());
        assert!(kept.birth.generation.is_none());
        let mut written = conversation();
        written.birth.generation = Some(OTHER.parse().expect("a time"));
        assert!(birth_kept(&directory, None, &written).is_err());
        // Nor is a found assistance written without it.
        let mut partial = conversation();
        partial.birth.born_under = Some(grant(GIVEN));
        assert!(birth_kept(&directory, None, &partial).is_err());
    }
}
