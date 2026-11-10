//! Synthetic membership is provisioned before the hub starts. Connections
//! still authenticate with real member keys over pinned TLS and WebSocket.

use std::collections::BTreeSet;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::Path;
use std::sync::Arc;

use fastctx_net::directory::{Account, Kind, Level, Member};
use fastctx_net::sign::SecretKey;
use fastctx_types::{AccountName, MachineName};

pub struct Identity {
    pub name: MachineName,
    pub account: AccountName,
    pub key: SecretKey,
    pub stable: bool,
    pub controller: bool,
    pub peer: MachineName,
}

pub fn prepare(
    root: &Path,
    members: usize,
    stable: usize,
) -> anyhow::Result<(SocketAddr, fastctx_net::ids::CertPin, Vec<Arc<Identity>>)> {
    let temporary = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    let address = temporary.local_addr()?;
    drop(temporary);
    let dir = root.join("hub");
    let pin = fastctx_hub::init(
        &dir,
        fastctx_hub::New {
            name: "scale".parse().map_err(anyhow::Error::msg)?,
            listen: address,
            public: vec![address],
            admin: "root".parse().map_err(anyhow::Error::msg)?,
            password: "Scale-drill-42!".into(),
        },
    )?;
    let file = dir.join("directory.json");
    let mut book: serde_json::Value = serde_json::from_slice(&std::fs::read(&file)?)?;
    let total = members + stable;
    let mut identities = Vec::new();
    let mut accounts = book["accounts"].as_array().expect("initial accounts").clone();
    for i in 0..total.div_ceil(2) {
        accounts.push(serde_json::to_value(Account {
            name: format!("a{i}").parse().map_err(anyhow::Error::msg)?,
            level: Level::Member,
            locked: false,
            opened: jiff::Timestamp::now(),
            collab: None,
            history: false,
            profile: None,
            promotion: false,
        })?);
    }
    let mut records = Vec::new();
    for i in 0..=total {
        let controller = i == total;
        let account: AccountName = if controller {
            "root".into()
        } else {
            format!("a{}", if i >= members { (i - members) / 2 } else { i / 2 })
        }
        .parse()
        .map_err(anyhow::Error::msg)?;
        let name: MachineName =
            if controller { "controller".into() } else { format!("m{i}") }.parse().map_err(anyhow::Error::msg)?;
        let key = SecretKey::generate()?;
        records.push(serde_json::to_value(Member {
            name: name.clone(),
            was: Vec::new(),
            key: key.public(),
            kind: Kind::Computer,
            logins: BTreeSet::from([account.clone()]),
            home: Some(account.clone()),
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
        })?);
        identities.push(Arc::new(Identity {
            name,
            account,
            key,
            stable: i >= members,
            controller,
            peer: format!(
                "m{}",
                if i >= members {
                    i - members
                } else if i % 2 == 0 {
                    i + 1
                } else {
                    i - 1
                }
            )
            .parse()
            .map_err(anyhow::Error::msg)?,
        }));
    }
    book["accounts"] = accounts.into();
    book["members"] = records.into();
    std::fs::write(file, serde_json::to_vec(&book)?)?;
    Ok((address, pin, identities))
}
