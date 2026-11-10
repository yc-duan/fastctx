use fastctx_net::{
    directory::{Kind, Member},
    ids::{CertPin, HubId},
    sign::SecretKey,
};
use fastctx_sync::{
    blocks::SqliteBlocks,
    catalog::Lifecycle,
    cursor::{Holder, Revision},
    identity::{AccountId, DeviceId, ObjectId, OperationId, ScopeKey},
    proof::{HubSigningIdentity, SigningKey},
    scope::{AccountIdentity, ScopeSubject, ScopeVisibility},
    store::{
        access::{AccessContext, AccessPolicy, Decision, HoldingUse},
        boundary, control,
        directory::{ScopeDescription, SqliteDirectory},
        schema,
    },
};
use std::{
    collections::BTreeSet,
    net::{Ipv4Addr, SocketAddr},
    path::Path,
    sync::Arc,
};
pub const CONVERSATIONS: usize = 70;
pub struct Fixture {
    pub address: SocketAddr,
    pub pin: CertPin,
    pub hub: HubId,
    pub key: SecretKey,
    pub identity: HubSigningIdentity,
}
pub fn blocks(root: &Path, holder: Holder) -> anyhow::Result<SqliteBlocks> {
    std::fs::create_dir_all(root)?;
    let db =
        fastctx_storage::Database::open_guarded(&root.join("node.sqlite3"), "", boundary::guard, schema::initialize)?;
    Ok(SqliteBlocks::new(
        db,
        root.join("blocks"),
        holder,
        Arc::new(Policy),
        OperationId::from_bytes([7; 32]),
        16 * 1024 * 1024,
    )?)
}
pub async fn prepare(root: &Path) -> anyhow::Result<Fixture> {
    std::fs::create_dir_all(root)?;
    let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    let address = listener.local_addr()?;
    drop(listener);
    let path = root.join("hub");
    let pin = fastctx_hub::init(
        &path,
        fastctx_hub::New {
            name: "private-test".parse().unwrap(),
            listen: address,
            public: vec![address],
            admin: "root".parse().unwrap(),
            password: "Private-fixture-42!".into(),
        },
    )?;
    let file = path.join("directory.json");
    let mut book: serde_json::Value = serde_json::from_slice(&std::fs::read(&file)?)?;
    let account: fastctx_net::directory::Account = serde_json::from_value(book["accounts"][0].clone())?;
    let hub_file: serde_json::Value = serde_json::from_slice(&std::fs::read(path.join("hub.json"))?)?;
    let hub: HubId = serde_json::from_value(hub_file["id"].clone())?;
    let key = SecretKey::generate()?;
    let member = Member {
        name: "node".parse().unwrap(),
        was: Vec::new(),
        key: key.public(),
        kind: Kind::Phone,
        logins: BTreeSet::from([account.name.clone()]),
        home: None,
        managed: false,
        code: None,
        group: None,
        direct: Vec::new(),
        relay: Vec::new(),
        added: jiff::Timestamp::now(),
        online: false,
        since: None,
        through: None,
        reach: None,
        paths: None,
        strain: None,
        workbench: None,
        settled: None,
    };
    book["members"] = serde_json::json!([member]);
    std::fs::write(file, serde_json::to_vec(&book)?)?;
    let service_key = SecretKey::generate()?;
    fastctx_platform::ensure_private_dir(&path.join("sync"))?;
    fastctx_platform::atomic::publish_new(
        &path.join("sync/signing.key"),
        &service_key.seed()[..],
        fastctx_platform::atomic::Permissions::Private,
    )?;
    let identity = HubSigningIdentity {
        hub: hub.clone(),
        key: SigningKey(*service_key.public().as_bytes()),
        revision: Revision(1),
        rotations: Vec::new(),
    };
    let db =
        fastctx_storage::Database::open_guarded(&path.join("hub.sqlite3"), "", boundary::guard, schema::initialize)?;
    let storage = SqliteBlocks::new(
        db,
        path.join("sync/blocks"),
        Holder::Hub { hub: hub.clone() },
        Arc::new(Policy),
        OperationId::from_bytes([6; 32]),
        16 * 1024 * 1024,
    )?;
    control::pin_key(&storage, &identity).await?;
    let run = control::AuthorityRun::begin(&storage, hub.clone(), Arc::new(fastctx_net::sign::random::<32>)).await?;
    let signer = Arc::new(fastctx_net::sync_proof::IdentitySigner::hub(
        hub.clone(),
        Arc::new(service_key),
        Arc::new(fastctx_sync::metrics::PlatformMetrics),
    ));
    let directory = SqliteDirectory::new(storage, run, signer);
    let owner = AccountIdentity {
        id: AccountId::from_legacy(&hub, &account.name, account.opened),
        name: account.name,
        opened: account.opened,
    };
    for n in 0..CONVERSATIONS {
        let mut bytes = [0; 32];
        bytes[..8].copy_from_slice(&(n as u64 + 1).to_be_bytes());
        let object = ObjectId::from_bytes(bytes);
        directory
            .publish(
                ScopeDescription {
                    scope: ScopeKey {
                        hub: hub.clone(),
                        scope: fastctx_types::sync::conversation_identity::conversation_scope(&object),
                    },
                    owner: owner.clone(),
                    subject: ScopeSubject::Conversation { conversation: object, shared_on: None, born_seal: None },
                    visibility: ScopeVisibility::Current,
                    acl_revision: Revision(1),
                    lifecycle: Lifecycle::Live,
                },
                OperationId::from_bytes(bytes),
            )
            .await?;
    }
    Ok(Fixture { address, pin, hub, key, identity })
}
struct Policy;
impl AccessPolicy for Policy {
    fn can_read(&self, _: &AccessContext, _: fastctx_sync::registry::RegisteredKind) -> Decision {
        Decision::Allowed { revision: Revision(1) }
    }
    fn can_read_object(
        &self,
        _: &AccessContext,
        _: fastctx_sync::registry::RegisteredKind,
        _: &fastctx_sync::identity::ObjectKey,
    ) -> Decision {
        Decision::Allowed { revision: Revision(1) }
    }
    fn can_write(&self, _: &AccessContext, _: fastctx_sync::registry::RegisteredKind) -> Decision {
        Decision::Allowed { revision: Revision(1) }
    }
    fn can_keep(
        &self,
        _: &AccessContext,
        _: fastctx_sync::registry::RegisteredKind,
        _: &fastctx_sync::identity::ObjectKey,
        _: &HoldingUse,
    ) -> Decision {
        Decision::Allowed { revision: Revision(1) }
    }
    fn can_execute(&self, _: &AccessContext, _: &DeviceId) -> Decision {
        Decision::Denied { revision: Revision(1), reason: "the discovery fixture cannot execute".into() }
    }
}
