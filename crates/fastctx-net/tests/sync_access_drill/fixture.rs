//! Ephemeral signing identities construct real scope evidence for policy checks.
use fastctx_net::{
    directory::{Account, Directory, Level, Member, Share},
    sign::{self, SecretKey},
    sync_access::{AccessState, DirectoryPolicy, DirectorySnapshot, MachineGrant, Snapshot},
    sync_proof::{IdentitySigner, IdentityVerifier},
};
use fastctx_sync::{
    catalog::{Lifecycle, SourceBatch, SourceRecord},
    cursor::{Holder, Revision, Sequence, VersionRef},
    identity::*,
    manifest::{BlobId, ContentBlock, DigestAlgorithm},
    metrics::PlatformMetrics,
    proof::*,
    scope::*,
};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
pub struct World {
    pub hub: HubId,
    pub identities: Vec<AccountIdentity>,
    pub directory: Directory,
    pub devices: [DeviceId; 2],
    signer: IdentitySigner,
    identity: HubSigningIdentity,
    verifier: IdentityVerifier,
    epoch: ProducerEpoch,
    next: AtomicU64,
}
struct State(Snapshot);
impl AccessState for State {
    fn current(&self, scope: &ScopeKey, object: Option<&ObjectKey>) -> Option<Snapshot> {
        (self.0.scope.descriptor().scope == *scope && object.is_none_or(|o| o.scope == *scope)).then(|| self.0.clone())
    }
}
impl World {
    pub fn new() -> Result<Self> {
        let hub = HubId::from_bytes(sign::random()?);
        let key = Arc::new(SecretKey::generate()?);
        let public = SigningKey(*key.public().as_bytes());
        let metrics = Arc::new(PlatformMetrics);
        let signer = IdentitySigner::hub(hub.clone(), key, metrics.clone());
        let identity =
            HubSigningIdentity { hub: hub.clone(), key: public.clone(), revision: Revision(1), rotations: vec![] };
        let verifier = IdentityVerifier::new(hub.clone(), [public], metrics);
        let mut directory = Directory::default();
        let mut identities = Vec::new();
        let keys = [SecretKey::generate()?, SecretKey::generate()?];
        let devices = keys.each_ref().map(|k| DeviceId::from_bytes(*k.public().as_bytes()));
        for (index, name) in ["owner", "reader"].into_iter().enumerate() {
            let name: fastctx_types::AccountName = name.parse()?;
            let opened = "2026-01-01T00:00:00Z".parse()?;
            identities.push(AccountIdentity {
                id: AccountId::from_legacy(&hub, &name, opened),
                name: name.clone(),
                opened,
            });
            directory.accounts.push(Arc::new(Account {
                name: name.clone(),
                level: if index == 0 { Level::Super } else { Level::Member },
                locked: false,
                opened,
                collab: None,
                history: false,
                profile: None,
                promotion: false,
            }));
            let member: Member = serde_json::from_value(
                serde_json::json!({"name":format!("machine-{index}"),"key":keys[index].public(),"kind":"computer","logins":[name],"home":name,"added":opened}),
            )?;
            directory.members.push(Arc::new(member));
        }
        directory.shares.push(Arc::new(Share {
            machine: "machine-0".parse()?,
            account: identities[1].name.clone(),
            jobs: None,
        }));
        Ok(Self {
            hub,
            identities,
            directory,
            devices,
            signer,
            identity,
            verifier,
            epoch: ProducerEpoch::from_bytes(sign::random()?),
            next: AtomicU64::new(0),
        })
    }
    pub fn descriptor(
        &self,
        number: u8,
        owner: ScopeOwner,
        subject: ScopeSubject,
        visibility: ScopeVisibility,
        acl: u64,
    ) -> ScopeDescriptor {
        ScopeDescriptor {
            format: 1,
            scope: ScopeKey { hub: self.hub.clone(), scope: ScopeId::from_bytes([number; 32]) },
            owner,
            subject,
            visibility,
            revision: VersionRef { authority_epoch: AuthorityEpoch::from_bytes([1; 16]), revision: Revision(acl) },
            acl_revision: Revision(acl),
            lifecycle: Lifecycle::Live,
        }
    }
    pub fn verify(&self, descriptor: ScopeDescriptor) -> Result<VerifiedScope> {
        let sequence = self.next.fetch_add(1, Ordering::SeqCst) + 1;
        let scope = ScopeKey { hub: self.hub.clone(), scope: ScopeId::from_bytes([200; 32]) };
        let operation = OperationId::from_bytes(Digest::of(&sequence.to_be_bytes()).bytes());
        let registration = OperationId::from_bytes([201; 32]);
        let epoch = EpochDescriptor {
            scope: scope.clone(),
            stream: StreamId::from_bytes([202; 32]),
            kind: "account.directory".into(),
            schema: 1,
            epoch: self.epoch.clone(),
            producer: Producer::Hub,
            predecessor: None,
            closed_through: None,
            operation: registration.clone(),
            control_sequence: Sequence(1),
            basis: HandoverBasis::Initial { registration },
        };
        let payload = serde_json::to_vec(&descriptor)?;
        let batch = SourceBatch {
            format: 1,
            scope: scope.clone(),
            stream: epoch.stream.clone(),
            producer: Producer::Hub,
            epoch: self.epoch.clone(),
            kind: "account.directory".into(),
            schema: 1,
            after: Sequence(sequence - 1),
            through: Sequence(sequence),
            records: vec![SourceRecord {
                sequence: Sequence(sequence),
                operation,
                kind: "account.directory".into(),
                schema: 1,
                payload: ContentBlock {
                    id: BlobId { domain: scope, algorithm: DigestAlgorithm::Sha256, digest: Digest::of(&payload) },
                    decoded_bytes: payload.len() as u32,
                },
                causes: vec![],
            }],
        };
        let proof = ScopeProof {
            epoch: self.signer.epoch(&epoch)?,
            batch: self.signer.source(&batch)?,
            payload: Bytes(payload),
        };
        Ok(fastctx_sync::scope::verify(&descriptor.scope, proof, &self.identity, &self.verifier)?)
    }
    pub fn policy(&self, scope: VerifiedScope, holder: Holder, reader_grant: Option<u64>) -> Result<DirectoryPolicy> {
        let mut grants = vec![MachineGrant {
            machine: self.devices[0].clone(),
            account: self.identities[0].id.clone(),
            revision: Revision(1),
        }];
        if let Some(revision) = reader_grant {
            grants.push(MachineGrant {
                machine: self.devices[0].clone(),
                account: self.identities[1].id.clone(),
                revision: Revision(revision),
            });
        }
        let directory =
            DirectorySnapshot::from_authenticated(self.hub.clone(), self.directory.clone(), self.identities.clone())?
                .with_grants(grants)?;
        Ok(DirectoryPolicy::from_authenticated(Arc::new(State(Snapshot {
            directory: Arc::new(directory),
            scope: Arc::new(scope),
            holder,
            enabled: true,
            source: None,
        }))))
    }
}
