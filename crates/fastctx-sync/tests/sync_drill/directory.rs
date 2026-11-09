//! A hub signs each scope description into its owner's directory stream; a
//! device discovers and verifies exactly the descriptions its accounts own.
#[path = "feed_reads.rs"]
mod feed_reads;
use super::*;
use fastctx_sync::{
    authority::{Peer, Service},
    catalog::{CatalogChange, Lifecycle, SourceBatch},
    cursor::Sequence,
    proof::{
        Bytes, CommittedFragment, ConfirmedEpoch, EpochDescriptor, HubKeyRotation, HubSigningIdentity, ProducerRelease,
        ProofPurpose, ProofSigner, ProofVerifier, Signature, SignedAcceptanceFragment, SignedSourceBatch,
        SignedStatement, SigningKey,
    },
    replicate::{BlockOffer, BlockRequest, DirectoryFollower, Upstream},
    scope::{
        self, AccountIdentity, ScopeDiscoveryRead, ScopeSubject, ScopeVisibility, account_scope, directory_object,
    },
    store::{
        Error,
        control::AuthorityRun,
        directory::{Recipient, ScopeDescription, SqliteDirectory},
        scopes::{Installed, ScopeState, SqliteScopes},
    },
    transfer::Message,
};
use fastctx_types::sync::{conversation_identity::conversation_scope, feed};

const HUB_KEY: [u8; 32] = [9; 32];

/// Signs hub statements with a placeholder signature the drill verifier trusts.
struct HubSigner;
impl ProofSigner for HubSigner {
    fn accepted(&self, _: &CommittedFragment) -> std::result::Result<SignedAcceptanceFragment, Error> {
        Err(Error::Unavailable("drill signer".into()))
    }
    fn source(&self, value: &SourceBatch) -> std::result::Result<SignedSourceBatch, Error> {
        Ok(SignedSourceBatch { proof: statement(ProofPurpose::HubSourceBatchV1, value)? })
    }
    fn epoch(&self, value: &EpochDescriptor) -> std::result::Result<ConfirmedEpoch, Error> {
        Ok(ConfirmedEpoch { proof: statement(ProofPurpose::EpochHandoverV1, value)? })
    }
    fn release(&self, _: &ProducerRelease) -> std::result::Result<SignedStatement, Error> {
        Err(Error::Unavailable("drill signer".into()))
    }
    fn exposure(
        &self,
        _: &fastctx_sync::exposure::ExposureReceipt,
    ) -> std::result::Result<fastctx_sync::exposure::SignedExposureReceipt, Error> {
        Err(Error::Unavailable("drill signer".into()))
    }
    fn rotation(&self, _: &HubKeyRotation) -> std::result::Result<SignedStatement, Error> {
        Err(Error::Unavailable("drill signer".into()))
    }
}
fn statement(purpose: ProofPurpose, value: &impl serde::Serialize) -> std::result::Result<SignedStatement, Error> {
    Ok(SignedStatement {
        purpose,
        signer: SigningKey(HUB_KEY),
        data: Bytes(serde_json::to_vec(value).map_err(|error| Error::Invalid(error.to_string()))?),
        signature: Signature([1; 64]),
    })
}

/// Trusts the drill signer's placeholder signatures but checks every binding.
struct Verifier;
impl ProofVerifier for Verifier {
    fn accepted(
        &self,
        _: &SignedAcceptanceFragment,
        _: &fastctx_sync::catalog::Authority,
    ) -> std::result::Result<CommittedFragment, Error> {
        Err(Error::Unavailable("drill verifier".into()))
    }
    fn source(&self, signed: &SignedSourceBatch, epoch: &EpochDescriptor) -> std::result::Result<SourceBatch, Error> {
        let batch: SourceBatch =
            serde_json::from_slice(&signed.proof.data.0).map_err(|error| Error::Invalid(error.to_string()))?;
        if signed.proof.purpose != ProofPurpose::HubSourceBatchV1
            || signed.proof.signer.0 != HUB_KEY
            || batch.scope != epoch.scope
            || batch.stream != epoch.stream
            || batch.epoch != epoch.epoch
            || batch.producer != epoch.producer
        {
            return Err(Error::Denied("source batch differs from its epoch".into()));
        }
        batch.validate_shape().map_err(Error::Invalid)?;
        Ok(batch)
    }
    fn epoch(
        &self,
        signed: &ConfirmedEpoch,
        identity: &HubSigningIdentity,
    ) -> std::result::Result<EpochDescriptor, Error> {
        if signed.proof.purpose != ProofPurpose::EpochHandoverV1 || signed.proof.signer != identity.key {
            return Err(Error::Denied("epoch is not signed by the pinned hub key".into()));
        }
        serde_json::from_slice(&signed.proof.data.0).map_err(|error| Error::Invalid(error.to_string()))
    }
    fn rotation(&self, _: &SignedStatement, _: &HubSigningIdentity) -> std::result::Result<HubKeyRotation, Error> {
        Err(Error::Unavailable("drill verifier".into()))
    }
}

/// A hub's own service may keep and read everything it describes.
struct HubPolicy;
impl HubPolicy {
    fn decide(&self, context: &AccessContext) -> Decision {
        match context.principal {
            Principal::LocalService => Decision::Allowed { revision: context.observed_acl },
            _ => Decision::Denied { revision: context.observed_acl, reason: "only the hub itself".into() },
        }
    }
}
impl AccessPolicy for HubPolicy {
    fn can_read(&self, c: &AccessContext, _: RegisteredKind) -> Decision {
        self.decide(c)
    }
    fn can_read_object(&self, c: &AccessContext, _: RegisteredKind, _: &ObjectKey) -> Decision {
        self.decide(c)
    }
    fn can_write(&self, c: &AccessContext, _: RegisteredKind) -> Decision {
        self.decide(c)
    }
    fn can_keep(&self, c: &AccessContext, _: RegisteredKind, _: &ObjectKey, _: &HoldingUse) -> Decision {
        self.decide(c)
    }
    fn can_execute(&self, c: &AccessContext, _: &DeviceId) -> Decision {
        Decision::Denied { revision: c.observed_acl, reason: "no execution".into() }
    }
}

/// Carries each request and answer through its JSON encoding, as a transport
/// would, to the hub's service for one authenticated device.
struct Loopback {
    service: Arc<Service>,
    peer: Peer,
}
impl Upstream for Loopback {
    fn exchange(&self, request: Message) -> fastctx_sync::store::FutureResult<'_, Message> {
        Box::pin(async move {
            let wire = |message: &Message| -> std::result::Result<Message, Error> {
                let bytes = serde_json::to_vec(message).map_err(|error| Error::Invalid(error.to_string()))?;
                serde_json::from_slice(&bytes).map_err(|error| Error::Invalid(error.to_string()))
            };
            let answer = self.service.answer(&self.peer, wire(&request)?).await?;
            wire(&answer)
        })
    }
    fn fetch(&self, _: BlockRequest) -> fastctx_sync::store::FutureResult<'_, Vec<u8>> {
        Box::pin(async { Err(Error::Unavailable("the loopback carries no blocks yet".into())) })
    }
    fn offer(&self, _: BlockOffer) -> fastctx_sync::store::FutureResult<'_, fastctx_sync::cursor::DurableAck> {
        Box::pin(async { Err(Error::Unavailable("the loopback carries no blocks yet".into())) })
    }
}

fn hub_store(dir: &Path, hub: &HubId) -> Result<(Database, SqliteBlocks)> {
    let db = Database::open_guarded(&dir.join("hub.sqlite3"), "", boundary::guard, schema::initialize)?;
    let blocks = SqliteBlocks::new(
        db.clone(),
        dir.join("files"),
        Holder::Hub { hub: hub.clone() },
        Arc::new(HubPolicy),
        operation(240),
        1 << 23,
    )?;
    Ok((db, blocks))
}
fn entropy(start: u64) -> fastctx_sync::store::control::Entropy {
    let next = Arc::new(std::sync::atomic::AtomicU64::new(start));
    Arc::new(move || {
        let mut bytes = [0; 32];
        bytes[..8].copy_from_slice(&next.fetch_add(1, Ordering::AcqRel).to_be_bytes());
        Ok(bytes)
    })
}
fn count(db: &Database, sql: &str) -> Result<i64> {
    Ok(db.read_now(|db| Ok(db.query_row(sql, [], |row| row.get(0))?))?)
}

pub(super) async fn run(root: &Path) -> Result<()> {
    let dir = root.join("directory");
    fastctx_platform::ensure_private_dir(&dir)?;
    let hub = HubId::from_bytes([31; 8]);
    let identity =
        HubSigningIdentity { hub: hub.clone(), key: SigningKey(HUB_KEY), revision: Revision(1), rotations: Vec::new() };
    let (db, blocks) = hub_store(&dir, &hub)?;
    let run = AuthorityRun::begin(&blocks, hub.clone(), entropy(1)).await?;
    let directory = SqliteDirectory::new(blocks.clone(), run, Arc::new(HubSigner));
    let name: fastctx_types::AccountName = "alice".parse()?;
    let opened = serde_json::from_str("\"2023-11-14T22:13:20Z\"")?;
    let owner = AccountIdentity { id: AccountId::from_legacy(&hub, &name, opened), name, opened };
    let conversation = ObjectId::from_bytes([33; 32]);
    let key = ScopeKey { hub: hub.clone(), scope: conversation_scope(&conversation) };
    let described = |acl: u64, lifecycle: Lifecycle| ScopeDescription {
        scope: key.clone(),
        owner: owner.clone(),
        subject: ScopeSubject::Conversation { conversation: conversation.clone(), shared_on: None, born_seal: None },
        visibility: ScopeVisibility::Current,
        acl_revision: Revision(acl),
        lifecycle,
    };

    // The owner's directory opens with its own description, then this one.
    let first = directory.publish(described(1, Lifecycle::Live), operation(241)).await?;
    let verified = scope::verify(&key, first.clone(), &identity, &Verifier)?;
    assert_eq!(verified.descriptor().acl_revision, Revision(1));
    assert_eq!(verified.source().committed, Sequence(2));
    let account = ScopeKey { hub: hub.clone(), scope: account_scope(&owner.id) };
    let directory_records =
        format!("SELECT COUNT(*) FROM sync_stream_records WHERE hub='{}' AND scope='{}'", hub, account.scope.as_str());
    assert_eq!(count(&db, &directory_records)?, 2);

    // A retried operation returns what it wrote; an unchanged description is
    // not written again under another operation.
    assert_eq!(directory.publish(described(1, Lifecycle::Live), operation(241)).await?, first);
    assert_eq!(directory.publish(described(1, Lifecycle::Live), operation(242)).await?, first);
    assert_eq!(count(&db, &directory_records)?, 2);

    // An ACL change is a new signed revision and advances the row's fence.
    let second = directory.publish(described(2, Lifecycle::Live), operation(243)).await?;
    let advanced = scope::verify(&key, second, &identity, &Verifier)?;
    advanced.validate_successor(&verified)?;
    assert_eq!(count(&db, "SELECT COUNT(*) FROM sync_scope_reconciliations")?, 1);
    let refused = |result: std::result::Result<_, Error>, why: &str| {
        assert!(matches!(&result, Err(Error::Invalid(reason)) if reason.contains(why)), "{why}: {:?}", result.err());
    };
    refused(directory.publish(described(3, Lifecycle::Live), operation(243)).await, "reused");
    refused(directory.publish(described(1, Lifecycle::Live), operation(244)).await, "ACL fence");
    let mut underived = described(3, Lifecycle::Live);
    underived.scope.scope = account_scope(&owner.id);
    refused(directory.publish(underived, operation(245)).await, "derive");
    let mut foreign = described(3, Lifecycle::Live);
    foreign.owner.id = AccountId::from_bytes([1; 32]);
    refused(directory.publish(foreign, operation(246)).await, "underived account");
    let mut project = described(3, Lifecycle::Live);
    project.subject = ScopeSubject::Project {
        project: conversation.clone(),
        town: fastctx_types::access::TownId::from_bytes([3; 8]),
    };
    assert!(matches!(directory.publish(project, operation(247)).await, Err(Error::Unavailable(_))));

    // The account scope's feed carries the directory stream's advances, each
    // anchored to the prefix before it.
    let changes = db.read_now(|db| {
        let mut rows = db.prepare(
            "SELECT canonical_change,prefix_digest FROM sync_changes WHERE hub=?1 AND scope=?2 ORDER BY sequence",
        )?;
        let found = rows
            .query_map(params![hub.to_string(), account.scope.as_str()], |row| {
                Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<fastctx_storage::rusqlite::Result<Vec<_>>>()?;
        let epoch: String = db.query_row(
            "SELECT epoch FROM sync_scope_heads WHERE hub=?1 AND scope=?2",
            params![hub.to_string(), account.scope.as_str()],
            |row| row.get(0),
        )?;
        Ok((found, epoch))
    })?;
    let (rows, epoch) = changes;
    assert_eq!(rows.len(), 3);
    let mut prefix = feed::genesis(&account, &FeedEpoch::try_from(epoch)?);
    for (canonical, stored) in rows {
        let change: CatalogChange = serde_json::from_slice(&canonical)?;
        assert_eq!(change.head.key, directory_object(&account));
        prefix = feed::advance(&prefix, &change)?.1;
        assert_eq!(prefix.as_str(), stored);
    }

    // Discovery pages through the recipient's directories in scope order and
    // hands over the directory cursors with the last page.
    let recipient = Recipient { device: DeviceId::from_bytes([34; 32]), accounts: vec![owner.id.clone()] };
    let request = |cursor| ScopeDiscoveryRead { cursor, directories: Vec::new(), max_items: 1, max_bytes: 256 * 1024 };
    let page = directory.discover(recipient.clone(), request(None)).await?;
    assert_eq!(page.entries.len(), 1);
    assert!(page.directories.is_empty());
    let next = page.next.clone().ok_or("discovery stopped after one page")?;
    let last = directory.discover(recipient.clone(), request(Some(next))).await?;
    assert_eq!(last.entries.len(), 1);
    assert!(last.next.is_none());
    assert_eq!(last.directories.len(), 1);
    assert_eq!(last.directories[0].through, Sequence(3));
    let mut seen = Vec::new();
    let mut account_scope_proof = None;
    for offer in page.entries.into_iter().chain(last.entries) {
        let scope = offer.cursor.scope.clone();
        let verified = scope::verify(&scope, offer.binding, &identity, &Verifier)?;
        assert!(!offer.kinds.is_empty());
        seen.push((scope.clone(), verified.descriptor().acl_revision));
        if scope == account {
            account_scope_proof = Some(verified);
        }
    }
    let mut expected = vec![(account.clone(), Revision(1)), (key.clone(), Revision(2))];
    expected.sort();
    assert_eq!(seen, expected);
    let stranger = Recipient { device: DeviceId::from_bytes([35; 32]), accounts: vec![AccountId::from_bytes([2; 32])] };
    assert!(directory.discover(stranger, request(None)).await?.entries.is_empty());

    // After a restart the hub runs under a new authority epoch; revisions are
    // new identities there and still succeed the earlier description.
    drop((directory, blocks));
    drop(db);
    super::super::scheduler::closed(&dir.join("hub.sqlite3")).await?;
    let (db, blocks) = hub_store(&dir, &hub)?;
    let run = AuthorityRun::begin(&blocks, hub.clone(), entropy(100)).await?;
    let directory = Arc::new(SqliteDirectory::new(blocks, run, Arc::new(HubSigner)));
    let third = directory.publish(described(3, Lifecycle::Deleted), operation(248)).await?;
    let deleted = scope::verify(&key, third, &identity, &Verifier)?;
    deleted.validate_successor(&advanced)?;
    assert_eq!(count(&db, &format!("SELECT enabled FROM sync_scopes WHERE scope='{}'", key.scope.as_str()))?, 0);

    // A device installs the hub's descriptions: the conversation it hosted
    // converts in place, a newer description reconciles custody, an older one
    // is refused, and an unrelated scope opens fresh.
    let device = DeviceId::from_bytes([36; 32]);
    let hosted = serde_json::to_vec(&serde_json::json!({
        "origin": "hosted",
        "subject": { "subject": "conversation", "conversation": conversation.as_str(), "shared_on": null, "born_seal": null },
        "device": device.as_str(),
    }))?;
    let node_db = Database::open_guarded(&dir.join("node.sqlite3"), "", boundary::guard, |db| {
        schema::initialize(db)?;
        db.execute(
            "INSERT INTO sync_scopes(hub,scope,origin,acl_revision,enabled,publication_epoch) VALUES(?1,?2,?3,?4,1,?4)",
            params![hub.to_string(), key.scope.as_str(), hosted, 1_u64.to_be_bytes().as_slice()],
        )?;
        Ok(())
    })?;
    let node_blocks = SqliteBlocks::new(
        node_db.clone(),
        dir.join("node-files"),
        Holder::Device { device: device.clone() },
        Arc::new(HubPolicy),
        operation(249),
        1 << 23,
    )?;
    let scopes = SqliteScopes::new(node_blocks, HubId::from_bytes([37; 8]), device.clone());
    let live = |acl: u64| ScopeState { acl_revision: Revision(acl), publication_epoch: Revision(1), enabled: true };
    assert_eq!(scopes.install(&verified).await?, Installed::Converted(live(1)));
    assert_eq!(scopes.install(&advanced).await?, Installed::Advanced(live(2)));
    assert_eq!(scopes.install(&advanced).await?, Installed::Unchanged(live(2)));
    assert!(matches!(scopes.install(&verified).await, Err(Error::Invalid(_))));
    assert_eq!(count(&node_db, "SELECT COUNT(*) FROM sync_scope_reconciliations")?, 1);
    let account_scope_proof = account_scope_proof.ok_or("discovery missed the account scope")?;
    assert_eq!(scopes.install(&account_scope_proof).await?, Installed::Opened(live(1)));
    let closed = ScopeState { acl_revision: Revision(3), publication_epoch: Revision(1), enabled: false };
    assert_eq!(scopes.install(&deleted).await?, Installed::Advanced(closed));

    // A fresh device learns the same scopes through the hub's service.
    let fresh = DeviceId::from_bytes([38; 32]);
    let fresh_db = Database::open_guarded(&dir.join("fresh.sqlite3"), "", boundary::guard, schema::initialize)?;
    let fresh_blocks = SqliteBlocks::new(
        fresh_db.clone(),
        dir.join("fresh-files"),
        Holder::Device { device: fresh.clone() },
        Arc::new(HubPolicy),
        operation(250),
        1 << 23,
    )?;
    let follower = DirectoryFollower::new(
        SqliteScopes::new(fresh_blocks, HubId::from_bytes([39; 8]), fresh.clone()),
        identity.clone(),
        Arc::new(Verifier),
    );
    let upstream =
        Loopback { service: Arc::new(Service::new(directory)), peer: Peer { device: fresh, accounts: vec![owner.id] } };
    let discovered = follower.discover(&upstream).await?;
    assert_eq!(discovered.installed.len(), 2);
    assert!(discovered.installed.iter().all(|installed| matches!(installed, Installed::Opened(_))));
    assert_eq!(discovered.directories.len(), 1);
    assert_eq!(count(&fresh_db, "SELECT COUNT(*) FROM sync_scopes WHERE enabled=0")?, 1);
    feed_reads::run(&upstream, &identity, &discovered.directories[0], &key).await?;
    println!(
        "hub directory: signed scope descriptions, idempotent retries, ACL advances, refusals, feed prefixes, paged discovery, restart and device installation passed"
    );
    Ok(())
}
