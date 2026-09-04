//! Routing at the hub: hub-terminated messages are applied to the store, member-to-member
//! messages are checked against grant shapes and forwarded on their headers.

use super::session::Peer;
use super::store::{InventoryRow, InviteRow, SealedKeyRow};
use super::{Hub, log};
use crate::world::crypto::b64_array;
use crate::world::envelope::{Envelope, Header};
use crate::world::grant::GrantSet;
use crate::world::messages::{self, SignedRevocation, kind};
use crate::world::{HUB_NAME, members, validate_node_name};
use std::collections::BTreeMap;

/// Handles a reliable message from `peer`. The caller acks whether or not this succeeds:
/// a message the hub cannot apply must not be retried forever.
pub(crate) fn handle_reliable(
    hub: &Hub,
    peer: &Peer,
    id: Option<u64>,
    env: Envelope,
) -> Result<(), String> {
    let header = env.header()?;
    check_author(peer, &header)?;
    if header.to == HUB_NAME {
        return handle_hub_bound(hub, peer, &header, &env);
    }
    forward_reliable(hub, peer, &header, id, env)
}

/// Handles a request or an answer from `peer`.
pub(crate) fn handle_request(
    hub: &Hub,
    peer: &Peer,
    id: Option<u64>,
    env: Envelope,
) -> Result<(), String> {
    let header = env.header()?;
    check_author(peer, &header)?;
    // A cancel withdraws a request the hub already holds, and it names that request inside its
    // own header, so it carries no transport id of its own: demanding one here would reject
    // every cancel and leave the call running on the target long after the caller gave up.
    if header.t == kind::CANCEL {
        hub.cancel_pending(&peer.name, header.id);
        return Ok(());
    }
    if header.to == HUB_NAME {
        let id =
            id.ok_or_else(|| format!("A {} request to the hub needs a transport id.", header.t))?;
        return answer_hub_request(hub, peer, id, &header, &env);
    }
    match header.t.as_str() {
        kind::CALL_RESULT => {
            let id =
                id.ok_or_else(|| "A call_result needs the transport id it answers.".to_string())?;
            hub.complete_pending(id, &peer.name, env);
            Ok(())
        }
        _ => {
            let id = id.ok_or_else(|| format!("A {} request needs a transport id.", header.t))?;
            forward_request(hub, peer, &header, id, env)
        }
    }
}

fn check_author(peer: &Peer, header: &Header) -> Result<(), String> {
    if header.from != peer.name {
        return Err(format!(
            "The envelope claims to be from \"{}\" but this connection belongs to \"{}\".",
            header.from, peer.name
        ));
    }
    Ok(())
}

fn require_signature(peer: &Peer, env: &Envelope, header: &Header) -> Result<(), String> {
    if messages::signature_required(&header.t) {
        env.verify_signature(&peer.node_pub)
            .map_err(|error| format!("{} must be signed by its author: {error}", header.t))?;
    }
    Ok(())
}

/// A revocation the connected member signed: its statement names the member as the revoker
/// and the signature verifies against the connection's key.
fn verify_peer_revocation(
    peer: &Peer,
    signed: &SignedRevocation,
) -> Result<messages::RevocationStatement, String> {
    let statement =
        members::verify_revocation(signed, |name| (name == peer.name).then_some(peer.node_pub))?;
    if statement.by != peer.name {
        return Err(format!(
            "the revocation names \"{}\" as the revoker but the connection belongs to \"{}\"",
            statement.by, peer.name
        ));
    }
    Ok(statement)
}

fn handle_hub_bound(hub: &Hub, peer: &Peer, header: &Header, env: &Envelope) -> Result<(), String> {
    require_signature(peer, env, header)?;
    match header.t.as_str() {
        kind::MEMBER_PUBLISH => {
            let opened = env.open(None)?;
            let body: messages::MemberPublish = messages::decode(&opened.body, &header.t)?;
            let record: messages::MemberRecord = serde_json::from_str(&body.signed.record)
                .map_err(|error| format!("member_publish carries an unreadable record: {error}"))?;
            if record.name != peer.name {
                return Err(format!(
                    "member_publish names \"{}\" but the connection belongs to \"{}\".",
                    record.name, peer.name
                ));
            }
            let mut row = hub
                .store
                .member(&peer.name)?
                .ok_or_else(|| "the member row vanished".to_string())?;
            if record.node_pub != row.node_pub {
                return Err(
                    "member_publish carries a public key other than the enrolled one.".to_string(),
                );
            }
            row.wrap_pub = record.wrap_pub.clone();
            row.tags = record.tags.clone();
            row.signed = Some(body.signed);
            let version = hub.store.put_member(&row)?;
            hub.broadcast_members_changed(version, Some(&peer.name));
            Ok(())
        }
        kind::INVITE_CREATE => {
            let opened = env.open(None)?;
            let body: messages::InviteCreate = messages::decode(&opened.body, &header.t)?;
            crate::world::parse_rfc3339(&body.exp)?;
            if body.code_id.len() != 64 || hex::decode(&body.code_id).is_err() {
                return Err("invite_create carries a malformed code id.".to_string());
            }
            hub.store.put_invite(
                &body.code_id,
                &InviteRow {
                    admission: body.admission,
                    wrapped_keys: body.wrapped_keys,
                    name: body.name.clone(),
                    exp: body.exp,
                    inviter: peer.name.clone(),
                    created_at: crate::world::now_rfc3339(),
                },
            )?;
            hub.append_event(
                &peer.name,
                "invite.created",
                [(
                    "name",
                    serde_json::Value::String(body.name.unwrap_or_default()),
                )],
            );
            Ok(())
        }
        kind::KEY_PUBLISH => {
            let opened = env.open(None)?;
            let body: messages::KeyPublish = messages::decode(&opened.body, &header.t)?;
            if body.epoch == 0 {
                return Err("key_publish needs an epoch above zero.".to_string());
            }
            let now = crate::world::now_rfc3339();
            // Each sealed key names its publisher and carries that publisher's signature;
            // the hub checks both against the connection so a member cannot publish keys in
            // another member's name, and members re-check the signature on receipt.
            for entry in &body.sealed {
                if entry.key.epoch != body.epoch {
                    return Err("key_publish mixes epochs.".to_string());
                }
                if entry.key.published_by != peer.name {
                    return Err(format!(
                        "key_publish names \"{}\" as the publisher but the connection belongs to \"{}\".",
                        entry.key.published_by, peer.name
                    ));
                }
                let Some(recipient) = hub.store.member(&entry.name)? else {
                    return Err(format!(
                        "key_publish seals a key for \"{}\", which is not a member.",
                        entry.name
                    ));
                };
                let wrap = b64_array::<32>(&recipient.wrap_pub, "member wrap key")?;
                entry.key.verify_publisher(&wrap, &peer.node_pub)?;
            }
            for entry in &body.sealed {
                hub.store.put_sealed_key(
                    body.epoch,
                    &entry.name,
                    &SealedKeyRow {
                        key: entry.key.clone(),
                        published_by: peer.name.clone(),
                        published_at: now.clone(),
                    },
                )?;
            }
            let newest = hub.store.meta_u64(super::store::meta::KEY_EPOCH)? as u32;
            if body.epoch > newest {
                hub.store
                    .set_meta_u64(super::store::meta::KEY_EPOCH, u64::from(body.epoch))?;
                hub.store
                    .remove_meta(super::store::meta::ROTATION_PENDING)?;
                hub.append_event(
                    &peer.name,
                    "key.rotated",
                    [
                        ("epoch", serde_json::Value::from(body.epoch)),
                        ("members", serde_json::Value::from(body.sealed.len())),
                    ],
                );
                let names = body
                    .sealed
                    .iter()
                    .map(|entry| entry.name.clone())
                    .collect::<Vec<_>>();
                hub.broadcast_reliable(
                    kind::KEY_ROTATED,
                    &messages::KeyRotated { epoch: body.epoch },
                    Some(&names),
                    Some(&peer.name),
                );
            }
            Ok(())
        }
        kind::INVENTORY => {
            if header.epoch == 0 {
                return Err("inventory must be encrypted.".to_string());
            }
            hub.store.put_inventory(
                &peer.name,
                &InventoryRow {
                    version: header.n,
                    envelope: env.clone(),
                    stored_at: crate::world::now_rfc3339(),
                },
            )?;
            Ok(())
        }
        kind::LEAVE => {
            let opened = env.open(None)?;
            let body: messages::Revoke = messages::decode(&opened.body, &header.t)?;
            let statement = verify_peer_revocation(peer, &body.revocation)?;
            if statement.name != peer.name {
                return Err(format!(
                    "leave must revoke the leaving member, not \"{}\".",
                    statement.name
                ));
            }
            hub.revoke(&peer.name, &peer.name, "left", Some(body.revocation))?;
            Ok(())
        }
        other => Err(format!(
            "The hub does not accept \"{other}\" as a reliable message."
        )),
    }
}

fn answer_hub_request(
    hub: &Hub,
    peer: &Peer,
    id: u64,
    header: &Header,
    env: &Envelope,
) -> Result<(), String> {
    require_signature(peer, env, header)?;
    match header.t.as_str() {
        kind::MEMBERS_GET => {
            let result = hub.members_result()?;
            hub.answer(&peer.name, id, kind::MEMBERS_RESULT, &result);
        }
        kind::INVENTORY_GET => {
            let opened = env.open(None)?;
            let body: messages::InventoryGet =
                messages::decode(&opened.body, &header.t).unwrap_or_default();
            let mut entries = Vec::new();
            for (name, row) in hub.store.inventories()? {
                if !body.names.is_empty() && !body.names.contains(&name) {
                    continue;
                }
                if body
                    .have
                    .get(&name)
                    .is_some_and(|version| *version >= row.version)
                {
                    continue;
                }
                entries.push(messages::InventoryEntry {
                    name,
                    version: row.version,
                    envelope: row.envelope,
                });
            }
            hub.answer(
                &peer.name,
                id,
                kind::INVENTORY_RESULT,
                &messages::InventoryResult { entries },
            );
        }
        kind::EVENTS_GET => {
            let opened = env.open(None)?;
            let body: messages::EventsGet = messages::decode(&opened.body, &header.t)?;
            let limit = body.limit.unwrap_or(200).clamp(1, 1000) as usize;
            let (events, latest) = hub.store.events_after(body.since, limit)?;
            hub.answer(
                &peer.name,
                id,
                kind::EVENTS_RESULT,
                &messages::EventsResult { events, latest },
            );
        }
        kind::GRANTS_GET => {
            // The snapshot also arrives unasked as a broadcast; this answers a member that
            // noticed it is behind, so a broadcast lost to a full outbox repairs itself.
            hub.answer(
                &peer.name,
                id,
                kind::GRANT_SYNC,
                &messages::GrantSync {
                    set: hub.store.grant_snapshot()?,
                },
            );
        }
        kind::GRANT_PUBLISH => {
            let opened = env.open(None)?;
            let body: messages::GrantPublish = messages::decode(&opened.body, &header.t)?;
            let snapshot = body.set.parse()?;
            if snapshot.published_by != peer.name {
                return Err(format!(
                    "the grant snapshot names \"{}\" as its publisher but the connection belongs to \"{}\".",
                    snapshot.published_by, peer.name
                ));
            }
            body.set.verify_signature(&peer.node_pub)?;
            match hub.store.put_grant_snapshot(&body.set, snapshot.revision) {
                Ok(()) => {}
                Err(error) if error.starts_with("grant_conflict:") => {
                    hub.send_hub_error(
                        &peer.name,
                        Some(id),
                        "grant_conflict",
                        error.trim_start_matches("grant_conflict: "),
                    );
                    return Ok(());
                }
                Err(error) => return Err(error),
            }
            hub.reload_grants();
            hub.append_event(
                &peer.name,
                "grant.changed",
                [
                    ("revision", serde_json::Value::from(snapshot.revision)),
                    ("grants", serde_json::Value::from(snapshot.grants.len())),
                ],
            );
            hub.broadcast_grant_sync(Some(&peer.name));
            let mut facts = BTreeMap::new();
            facts.insert(
                "revision".to_string(),
                serde_json::Value::from(snapshot.revision),
            );
            hub.answer(
                &peer.name,
                id,
                kind::HUB_RESULT,
                &messages::HubResult { facts },
            );
        }
        kind::KEYS_GET => {
            let opened = env.open(None)?;
            let body: messages::KeysGet = messages::decode(&opened.body, &header.t)?;
            let sealed = hub
                .store
                .sealed_keys_for(&peer.name)?
                .into_iter()
                .filter(|(epoch, _)| !body.have.contains(epoch))
                .map(|(_, row)| row.key)
                .collect();
            let newest_epoch = hub.store.meta_u64(super::store::meta::KEY_EPOCH)? as u32;
            hub.answer(
                &peer.name,
                id,
                kind::KEYS_RESULT,
                &messages::KeysResult {
                    sealed,
                    newest_epoch,
                },
            );
        }
        kind::REVOKE => {
            let opened = env.open(None)?;
            let body: messages::Revoke = messages::decode(&opened.body, &header.t)?;
            let statement = verify_peer_revocation(peer, &body.revocation)?;
            validate_node_name(&statement.name)?;
            let Some(row) = hub.store.member(&statement.name)? else {
                return Err(format!("No member named \"{}\".", statement.name));
            };
            if row.node_pub != statement.node_pub {
                return Err(format!(
                    "The revocation names a key that is not \"{}\"'s enrolled key.",
                    statement.name
                ));
            }
            hub.revoke(
                &statement.name,
                &peer.name,
                &statement.reason,
                Some(body.revocation),
            )?;
            hub.answer(
                &peer.name,
                id,
                kind::HUB_RESULT,
                &messages::HubResult::default(),
            );
        }
        other => return Err(format!("The hub does not answer \"{other}\" requests.")),
    }
    Ok(())
}

/// Forwards a reliable member-to-member message through the target's outbox.
fn forward_reliable(
    hub: &Hub,
    peer: &Peer,
    header: &Header,
    id: Option<u64>,
    env: Envelope,
) -> Result<(), String> {
    if header.epoch == 0 {
        return Err(format!(
            "A {} message between members must be encrypted.",
            header.t
        ));
    }
    let targets = targets_of(header);
    for target in targets {
        hub.check_grant(&peer.name, header.verb.as_deref(), &target)?;
        hub.queue_reliable(&target, env.clone(), id)?;
    }
    Ok(())
}

/// Forwards a request to every target, answering on the hub's behalf where a target cannot
/// receive it, and telling the caller which legs were handed over so it can later tell an
/// undelivered leg from one whose outcome is unknown.
fn forward_request(
    hub: &Hub,
    peer: &Peer,
    header: &Header,
    id: u64,
    env: Envelope,
) -> Result<(), String> {
    if header.epoch == 0 {
        return Err(format!(
            "A {} request between members must be encrypted.",
            header.t
        ));
    }
    for target in targets_of(header) {
        // Each message completes "<node>: <status> (…)" on the caller, so it names neither the
        // node nor the status again.
        let status = match hub.check_grant(&peer.name, header.verb.as_deref(), &target) {
            Err(_) => Some((
                "forbidden",
                match header.verb.as_deref() {
                    Some(verb) => {
                        format!("no grant allows {verb} here for \"{}\"", peer.name)
                    }
                    None => format!("no grant allows this call from \"{}\"", peer.name),
                },
            )),
            Ok(()) => match hub.store.member(&target)? {
                None => Some(("unknown", "no member with this name".to_string())),
                Some(row) if row.is_revoked() => {
                    Some(("revoked", "this member was revoked".to_string()))
                }
                Some(_) if !hub.is_online(&target) => Some((
                    "offline",
                    format!("last seen {}", hub.last_seen_text(&target)),
                )),
                Some(_) => None,
            },
        };
        match status {
            Some((status, message)) => hub.answer(
                &peer.name,
                id,
                kind::CALL_STATUS,
                &messages::CallStatus {
                    node: target.clone(),
                    status: status.to_string(),
                    message,
                },
            ),
            None => {
                let hub_id = hub.register_pending(&peer.name, id, &target);
                if hub.send_to_online(
                    &target,
                    crate::world::wire::Frame::request(hub_id, env.clone()),
                ) {
                    hub.answer(
                        &peer.name,
                        id,
                        kind::CALL_STATUS,
                        &messages::CallStatus {
                            node: target.clone(),
                            status: messages::CALL_DELIVERED.to_string(),
                            message: String::new(),
                        },
                    );
                } else {
                    hub.forget_pending(hub_id);
                    hub.answer(
                        &peer.name,
                        id,
                        kind::CALL_STATUS,
                        &messages::CallStatus {
                            node: target.clone(),
                            status: "offline".to_string(),
                            message: format!("Node \"{target}\" went offline while the request was being delivered."),
                        },
                    );
                }
            }
        }
    }
    Ok(())
}

fn targets_of(header: &Header) -> Vec<String> {
    match &header.targets {
        Some(targets) if !targets.is_empty() => targets.clone(),
        _ => vec![header.to.clone()],
    }
}

impl Hub {
    /// Grant shapes the hub enforces, from the snapshot in force: the publisher's signature
    /// was checked when it was stored, the MAC is not (the hub has no World key); members
    /// re-check both on their copies.
    pub(crate) fn reload_grants(&self) {
        let set = match self.store.grant_snapshot() {
            Ok(Some(signed)) => match signed.parse() {
                Ok(snapshot) => GrantSet {
                    revision: snapshot.revision,
                    grants: snapshot.grants,
                    published_by: snapshot.published_by,
                    published_at: snapshot.published_at,
                    signed: Some(signed),
                },
                Err(error) => {
                    log(format!(
                        "the stored grant snapshot is unreadable and is ignored: {error}"
                    ));
                    GrantSet::default()
                }
            },
            Ok(None) => GrantSet::default(),
            Err(error) => {
                log(format!("cannot load grants: {error}"));
                return;
            }
        };
        *self.grants.lock() = set;
    }

    pub(crate) fn check_grant(
        &self,
        principal: &str,
        verb: Option<&str>,
        target: &str,
    ) -> Result<(), String> {
        let Some(verb) = verb else {
            return Ok(());
        };
        let tags = self
            .store
            .member(target)?
            .map(|row| row.tags)
            .unwrap_or_default();
        if self.grants.lock().allows(principal, verb, target, &tags) {
            Ok(())
        } else {
            Err(format!(
                "forbidden: node \"{target}\" does not allow {verb} for \"{principal}\"."
            ))
        }
    }

    /// Every member the hub knows, revoked ones included, plus who lacks the newest key.
    pub(crate) fn members_result(&self) -> Result<messages::MembersResult, String> {
        let sessions = self.store.sessions()?;
        let mut members = Vec::new();
        for row in self.store.members()? {
            let session = sessions.get(&row.name).cloned().unwrap_or_default();
            members.push(messages::MemberEntry {
                state: if row.is_revoked() {
                    members::STATE_REVOKED
                } else if self.is_online(&row.name) {
                    "online"
                } else {
                    "offline"
                }
                .to_string(),
                name: row.name,
                signed: row.signed,
                last_seen: session.last_seen,
                hub_rtt_ms: session.rtt_ms,
                tls: session.tls,
                network: session.network,
                version: (!session.version.is_empty()).then_some(session.version),
                inventory_version: session.inventory_version,
                revocation: row.revocation,
            });
        }
        let key_epoch = self.store.meta_u64(super::store::meta::KEY_EPOCH)? as u32;
        let missing_key = if key_epoch > 0 {
            self.store.members_missing_epoch(key_epoch)?
        } else {
            Vec::new()
        };
        Ok(messages::MembersResult {
            version: self.store.meta_u64(super::store::meta::MEMBERS_VERSION)?,
            members,
            missing_key,
            key_epoch,
        })
    }

    /// Revokes a member: admission removed, keys and inventory dropped, connection closed,
    /// rotation flagged for the next member able to rotate. `revocation` is the member-signed
    /// statement when a member asked; the hub operator has none, and the next member to
    /// rotate countersigns for them.
    pub(crate) fn revoke(
        &self,
        name: &str,
        by: &str,
        reason: &str,
        revocation: Option<SignedRevocation>,
    ) -> Result<(), String> {
        let Some(mut row) = self.store.member(name)? else {
            return Err(format!("No member named \"{name}\"."));
        };
        if row.is_revoked() {
            if row.revocation.is_none() && revocation.is_some() {
                row.revocation = revocation;
                let version = self.store.put_member(&row)?;
                self.broadcast_members_changed(version, None);
                log(format!("\"{name}\": revocation countersigned by {by}"));
            }
            return Ok(());
        }
        row.revoked_at = Some(crate::world::now_rfc3339());
        row.revoke_reason = Some(reason.to_string());
        row.revocation = revocation;
        let version = self.store.put_member(&row)?;
        self.store.remove_sealed_keys_for(name)?;
        self.store.remove_inventory(name)?;
        self.store.outbox_clear(name)?;
        if reason != "left" {
            self.store
                .set_meta_u64(super::store::meta::ROTATION_PENDING, 1)?;
        }
        self.append_event(
            name,
            "node.revoked",
            [
                ("by", serde_json::Value::String(by.to_string())),
                ("reason", serde_json::Value::String(reason.to_string())),
            ],
        );
        if let Some(connection) = self.connection(name) {
            let env = self.hub_envelope(
                kind::REVOKED,
                name,
                None,
                &messages::Revoked {
                    name: name.to_string(),
                    reason: reason.to_string(),
                },
            );
            if let Ok(env) = env {
                let _ = connection.tx.send(crate::world::wire::Frame::Msg {
                    seq: None,
                    id: None,
                    env,
                });
            }
            let _ = connection.tx.send(crate::world::wire::Frame::Bye {
                reason: "revoked".to_string(),
            });
            connection.cancel.cancel();
        }
        self.broadcast_members_changed(version, Some(name));
        log(format!("\"{name}\" revoked by {by} ({reason})"));
        Ok(())
    }

    pub(crate) fn broadcast_members_changed(&self, version: u64, except: Option<&str>) {
        self.broadcast_reliable(
            kind::MEMBERS_CHANGED,
            &messages::MembersChanged { version },
            None,
            except,
        );
    }

    pub(crate) fn broadcast_grant_sync(&self, except: Option<&str>) {
        let set = match self.store.grant_snapshot() {
            Ok(set) => set,
            Err(error) => {
                log(format!("cannot broadcast grants: {error}"));
                return;
            }
        };
        self.broadcast_reliable(kind::GRANT_SYNC, &messages::GrantSync { set }, None, except);
    }

    /// Queues one reliable hub message for every enrolled member (or the named subset).
    pub(crate) fn broadcast_reliable<T: serde::Serialize>(
        &self,
        t: &str,
        body: &T,
        only: Option<&[String]>,
        except: Option<&str>,
    ) {
        let members = match self.store.members() {
            Ok(members) => members,
            Err(error) => {
                log(format!("cannot broadcast {t}: {error}"));
                return;
            }
        };
        for member in members {
            if member.is_revoked() || except == Some(member.name.as_str()) {
                continue;
            }
            if let Some(only) = only
                && !only.contains(&member.name)
            {
                continue;
            }
            match self.hub_envelope(t, &member.name, None, body) {
                Ok(env) => {
                    if let Err(error) = self.queue_reliable(&member.name, env, None) {
                        log(format!("cannot queue {t} for \"{}\": {error}", member.name));
                    }
                }
                Err(error) => log(format!("cannot build {t}: {error}")),
            }
        }
    }

    /// Answers request `id` from `to` with a hub-originated envelope whose header carries the
    /// same id, so the recipient can hold the hub to the request it is answering.
    pub(crate) fn answer<T: serde::Serialize>(&self, to: &str, id: u64, t: &str, body: &T) {
        match self.hub_envelope(t, to, Some(id), body) {
            Ok(env) => {
                self.send_to_online(to, crate::world::wire::Frame::request(id, env));
            }
            Err(error) => log(format!("cannot build {t}: {error}")),
        }
    }

    pub(crate) fn send_hub_error(&self, to: &str, id: Option<u64>, code: &str, message: &str) {
        let body = messages::HubError {
            code: code.to_string(),
            message: message.to_string(),
        };
        if let Ok(env) = self.hub_envelope(kind::HUB_ERROR, to, id, &body) {
            self.send_to_online(to, crate::world::wire::Frame::Msg { seq: None, id, env });
        }
    }

    pub(crate) fn hub_envelope<T: serde::Serialize>(
        &self,
        t: &str,
        to: &str,
        id: Option<u64>,
        body: &T,
    ) -> Result<Envelope, String> {
        let mut header = Header::new(t, HUB_NAME, to, self.next_n());
        if let Some(id) = id {
            header = header.with_id(id);
        }
        Envelope::seal_plain(header, &messages::encode(body)?)
    }

    pub(crate) fn append_event<const N: usize>(
        &self,
        subject: &str,
        kind: &str,
        facts: [(&str, serde_json::Value); N],
    ) {
        let facts = facts
            .into_iter()
            .map(|(key, value)| (key.to_string(), value))
            .collect::<BTreeMap<_, _>>();
        if let Err(error) = self.store.append_event(subject, kind, facts, None) {
            log(format!("cannot append event {kind}: {error}"));
        }
    }
}
