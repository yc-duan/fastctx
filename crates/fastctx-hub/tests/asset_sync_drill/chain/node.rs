use super::*;
use fastctx_sync::{
    blocks::SqliteBlocks,
    cursor::*,
    identity::*,
    store::{access::*, boundary, schema, scopes::SqliteScopes},
};

pub(super) fn blocks(
    root: &Path,
    config: &Config,
    n: usize,
) -> anyhow::Result<(fastctx_storage::Database, SqliteBlocks)> {
    fastctx_platform::ensure_private_dir(root)?;
    let conversation = config.conversation();
    let device = config.device(n);
    let origin = serde_json::to_vec(&serde_json::json!({ "origin": "hosted",
        "subject": { "subject": "conversation", "conversation": conversation.object,
            "shared_on": null, "born_seal": null }, "device": device }))?;
    let db = fastctx_storage::Database::open_guarded(&root.join("node.sqlite3"), "", boundary::guard, move |db| {
        schema::initialize(db)?;
        if n == 0 {
            db.execute(
                "INSERT INTO sync_scopes(hub,scope,origin,acl_revision,enabled,publication_epoch)
             VALUES(?1,?2,?3,?4,1,?4) ON CONFLICT DO NOTHING",
                fastctx_storage::rusqlite::params![
                    conversation.scope.hub.to_string(),
                    conversation.scope.scope.as_str(),
                    origin,
                    1_u64.to_be_bytes().as_slice()
                ],
            )?;
        }
        Ok(())
    })?;
    let blocks = SqliteBlocks::new(
        db.clone(),
        root.join("blocks"),
        Holder::Device { device: config.device(n) },
        Arc::new(policy::Policy(config.account.id.clone(), config.device(0))),
        OperationId::from_bytes([n as u8 + 20; 32]),
        32 * 1024 * 1024,
    )?;
    Ok((db, blocks))
}
pub(super) async fn connect(
    root: &Path,
    config: &Config,
    n: usize,
    blocks: &SqliteBlocks,
) -> anyhow::Result<(fastctx_net::link::Link, Arc<fastctx_net::link::AssetUpstream>)> {
    let membership = fastctx_net::link::Membership {
        id: config.hub.clone(),
        hub_name: "private-test".parse()?,
        name: if n == 0 { "node".parse()? } else { format!("replica-{n}").parse()? },
        addresses: vec![config.address],
        cert: config.pin,
        kind: fastctx_net::directory::Kind::Phone,
        relays: Vec::new(),
    };
    let link = fastctx_net::link::Link::start_managed(
        fastctx_net::link::NetDir::new(root.join("link")),
        membership,
        config.key(n),
        Arc::new(crate::node::Node),
    )?;
    let upstream = tokio::time::timeout(Duration::from_secs(60), link.asset_upstream(blocks.buffer_budget())).await??;
    anyhow::ensure!(upstream.identity() == &config.identity, "unexpected authenticated hub key");
    fastctx_sync::store::control::pin_key(blocks, upstream.identity()).await?;
    Ok((link, Arc::new(upstream)))
}
pub(super) fn directory(
    blocks: &SqliteBlocks,
    config: &Config,
    n: usize,
) -> fastctx_sync::replicate::DirectoryFollower {
    fastctx_sync::replicate::DirectoryFollower::new(
        SqliteScopes::new(blocks.clone(), HubId::from_bytes([77; 8]), config.device(n)),
        config.identity.clone(),
        config.verifier(),
    )
}
pub(super) fn holding(config: &Config, n: usize) -> HoldingPeriod {
    HoldingPeriod {
        scope: config.conversation().scope,
        holder: Holder::Device { device: config.device(n) },
        acl_revision: Revision(1),
        publication_epoch: Revision(1),
    }
}
pub(super) fn context(config: &Config, n: usize) -> AccessContext {
    AccessContext {
        principal: Principal::Account { account: config.account.id.clone(), device: config.device(n) },
        scope: config.conversation().scope,
        observed_acl: Revision(1),
    }
}
