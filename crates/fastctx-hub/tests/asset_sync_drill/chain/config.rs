use super::*;
use fastctx_sync::{identity::*, proof::HubSigningIdentity, scope::AccountIdentity};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub(super) struct Config {
    pub address: std::net::SocketAddr,
    pub pin: fastctx_net::ids::CertPin,
    pub hub: fastctx_net::ids::HubId,
    pub identity: HubSigningIdentity,
    pub account: AccountIdentity,
    pub seeds: Vec<[u8; 32]>,
}
impl Config {
    pub async fn prepare(root: &Path) -> anyhow::Result<Self> {
        let fixture = crate::fixture::prepare(root).await?;
        let path = root.join("hub/directory.json");
        let mut book: serde_json::Value = serde_json::from_slice(&std::fs::read(&path)?)?;
        let account: fastctx_net::directory::Account = serde_json::from_value(book["accounts"][0].clone())?;
        let mut secondary = book["accounts"][0].clone();
        secondary["name"] = serde_json::json!("observer");
        book["accounts"].as_array_mut().unwrap().push(secondary);
        let mut seeds = vec![*fixture.key.seed()];
        for n in 1..=4 {
            let key = fastctx_net::sign::SecretKey::generate()?;
            seeds.push(*key.seed());
            let mut member = book["members"][0].clone();
            member["name"] = serde_json::json!(format!("replica-{n}"));
            member["key"] = serde_json::to_value(key.public())?;
            // A second login keeps the control session alive after root logout,
            // so the refusal exercises per-scope authorization, not disconnect.
            member["logins"] = serde_json::json!([account.name, "observer"]);
            book["members"].as_array_mut().unwrap().push(member);
        }
        std::fs::write(path, serde_json::to_vec(&book)?)?;
        let config = Self {
            address: fixture.address,
            pin: fixture.pin,
            hub: fixture.hub.clone(),
            identity: fixture.identity,
            account: AccountIdentity {
                id: AccountId::from_legacy(&fixture.hub, &account.name, account.opened),
                name: account.name,
                opened: account.opened,
            },
            seeds,
        };
        config.save(root)?;
        Ok(config)
    }
    pub fn load(root: &Path) -> anyhow::Result<Self> {
        Ok(serde_json::from_slice(&std::fs::read(root.join("chain.json"))?)?)
    }
    pub fn save(&self, root: &Path) -> anyhow::Result<()> {
        Ok(fastctx_platform::atomic::publish_new(
            &root.join("chain.json"),
            &serde_json::to_vec(self)?,
            fastctx_platform::atomic::Permissions::Private,
        )?)
    }
    pub fn key(&self, n: usize) -> fastctx_net::sign::SecretKey {
        fastctx_net::sign::SecretKey::from_seed(&self.seeds[n])
    }
    pub fn device(&self, n: usize) -> DeviceId {
        DeviceId::from_bytes(*self.key(n).public().as_bytes())
    }
    pub fn conversation(&self) -> ObjectKey {
        let mut bytes = [0; 32];
        bytes[..8].copy_from_slice(&1_u64.to_be_bytes());
        let object = ObjectId::from_bytes(bytes);
        ObjectKey {
            scope: ScopeKey {
                hub: self.hub.clone(),
                scope: fastctx_types::sync::conversation_identity::conversation_scope(&object),
            },
            object,
        }
    }
    pub fn object(&self) -> ObjectKey {
        let c = self.conversation();
        ObjectKey {
            scope: c.scope,
            object: fastctx_types::sync::conversation_identity::source_object(&c.object, KIND).unwrap(),
        }
    }
    pub fn verifier(&self) -> Arc<fastctx_net::sync_proof::IdentityVerifier> {
        Arc::new(fastctx_net::sync_proof::IdentityVerifier::new(
            self.hub.clone(),
            [self.identity.key.clone()],
            Arc::new(fastctx_sync::metrics::PlatformMetrics),
        ))
    }
    pub fn installation(&self) -> fastctx_hub::FoundationInstallation {
        fastctx_hub::FoundationInstallation {
            access: Arc::new(policy::Policy(self.account.id.clone(), self.device(0))),
            sources: vec![Arc::new(policy::Gate(self.conversation()))],
        }
    }
}
