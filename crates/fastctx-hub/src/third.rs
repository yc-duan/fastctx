//! A hub of the network's third version, made one of the fourth. Its id and
//! certificate stay, so every computer that pinned them reconnects with the
//! key it has; its machines become machines of one account, its devices
//! logins of that account. What it kept of the projects' ledgers and of
//! letters stays behind: the computers carry their projects themselves
//! (`fastctx_node::ledger::third`).

use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::path::Path;

use fastctx_net::directory::{Account, Kind, Member};
use fastctx_net::ids::HubId;
use fastctx_net::login::HubSecrets;
use fastctx_net::sign::{Bytes, PublicKey, Signed};
use fastctx_net::tls::HubCertificate;
use fastctx_types::{AccountName, HubName, MachineName};
use jiff::Timestamp;
use serde::Deserialize;
use serde_json::Value;

use crate::store::{Book, HubFile, Store};

/// What the fourth version's hub is made with.
pub struct Migrated {
    pub name: HubName,
    /// The account the members go into, the super administrator, and its password.
    pub admin: AccountName,
    pub password: String,
}

/// What became of the third version's members.
#[derive(Debug, Default)]
pub struct Report {
    pub machines: Vec<MachineName>,
    pub devices: Vec<MachineName>,
    /// Revoked, or not readable, with why.
    pub left: Vec<(MachineName, String)>,
    /// Letters the third version still held, which do not come along.
    pub letters: usize,
}

#[derive(Deserialize)]
struct ThirdHub {
    world: String,
    public: Vec<SocketAddr>,
    listen: SocketAddr,
}

#[derive(Default, Deserialize)]
struct ThirdMembers {
    records: Vec<ThirdAdmitted>,
    #[serde(default)]
    revocations: Vec<Signed>,
}

#[derive(Deserialize)]
struct ThirdAdmitted {
    name: MachineName,
    record: Signed,
    joined_at: Timestamp,
}

/// Makes the hub kept in `from`, of the third version, a hub of the fourth
/// in `to`, which must hold none. `from` is read, never changed.
pub fn migrate(from: &Path, to: &Path, new: Migrated) -> anyhow::Result<Report> {
    if Store::hub_file(to).exists() {
        anyhow::bail!("{} already holds a hub", to.display());
    }
    let third: ThirdHub = read(&from.join("hub.json"))?;
    let id = HubId::try_from(third.world.clone()).map_err(anyhow::Error::msg)?;
    let certificate = HubCertificate {
        certificate: std::fs::read(from.join("tls").join("certificate.der"))?,
        key: std::fs::read(from.join("tls").join("key.der"))?,
    };
    let members: ThirdMembers = match std::fs::read(from.join("members.json")) {
        Ok(bytes) => serde_json::from_slice(&bytes)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => ThirdMembers::default(),
        Err(error) => return Err(error.into()),
    };
    let revoked: BTreeSet<String> = members
        .revocations
        .iter()
        .filter_map(|signed| signed.claim::<Value>().ok())
        .filter_map(|revocation| revocation.get("key").and_then(Value::as_str).map(str::to_string))
        .collect();
    let secrets = HubSecrets::generate();
    let mut book = Book::default();
    book.directory.accounts.push(Account {
        name: new.admin.clone(),
        level: fastctx_net::directory::Level::Super,
        locked: false,
        opened: Timestamp::now(),
        collab: None,
        profile: None,
        promotion: false,
    });
    book.passwords.insert(new.admin.clone(), Bytes(crate::password_record(&secrets, &new.admin, &new.password)?));
    let mut report = Report::default();
    for admitted in members.records {
        let name = admitted.name.clone();
        let (key, machine) = match standing(&admitted.record) {
            Ok(standing) => standing,
            Err(why) => {
                report.left.push((name, why));
                continue;
            }
        };
        if revoked.contains(&key.to_string()) {
            report.left.push((name, "it was revoked".to_string()));
            continue;
        }
        if book.directory.members.iter().any(|member| member.name == name || member.key == key) {
            report.left.push((name, "a member of this name or key came first".to_string()));
            continue;
        }
        book.directory.members.push(Member {
            name: name.clone(),
            was: Vec::new(),
            key,
            kind: if machine { Kind::Computer } else { Kind::Phone },
            logins: BTreeSet::from([new.admin.clone()]),
            home: machine.then(|| new.admin.clone()),
            managed: false,
            code: None,
            group: None,
            direct: Vec::new(),
            relay: Vec::new(),
            added: admitted.joined_at,
            online: false,
            since: None,
            through: None,
            reach: None,
            paths: None,
            settled: None,
        });
        if machine { report.machines.push(name) } else { report.devices.push(name) }
    }
    report.letters = count_files(&from.join("outbox"))? + count_files(&from.join("outboxes"))?;
    let hub = HubFile {
        id,
        name: new.name,
        public: third.public,
        listen: third.listen,
        self_registration: false,
        dissolved: None,
        updates: Default::default(),
        due: Vec::new(),
        repairs: Vec::new(),
        package_targets: Default::default(),
    };
    Store::create(to, &hub, &certificate, &secrets.to_bytes(), &book)?;
    Ok(report)
}

/// A member's key, and whether it is a machine, from its record: devices
/// were invited as such, and everyone else is a machine.
fn standing(record: &Signed) -> Result<(PublicKey, bool), String> {
    let record: Value = record.claim().map_err(|error| format!("its record cannot be read: {error}"))?;
    let key: PublicKey = serde_json::from_value(record.get("key").cloned().unwrap_or(Value::Null))
        .map_err(|error| format!("its key cannot be read: {error}"))?;
    let kind = record
        .get("credential")
        .and_then(|credential| credential.get("admission"))
        .and_then(|admission| serde_json::from_value::<Signed>(admission.clone()).ok())
        .and_then(|admission| admission.claim::<Value>().ok())
        .and_then(|admission| admission.get("endorsement").cloned())
        .and_then(|endorsement| serde_json::from_value::<Signed>(endorsement).ok())
        .and_then(|endorsement| endorsement.claim::<Value>().ok())
        .and_then(|endorsement| endorsement.get("kind").and_then(Value::as_str).map(str::to_string));
    Ok((key, kind.as_deref() != Some("device")))
}

fn read<T: serde::de::DeserializeOwned>(path: &Path) -> anyhow::Result<T> {
    let bytes = std::fs::read(path).map_err(|error| anyhow::anyhow!("reading {}: {error}", path.display()))?;
    serde_json::from_slice(&bytes).map_err(|error| anyhow::anyhow!("reading {}: {error}", path.display()))
}

fn count_files(directory: &Path) -> std::io::Result<usize> {
    let mut count = 0;
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = entry?;
        count += if entry.file_type()?.is_dir() { count_files(&entry.path())? } else { 1 };
    }
    Ok(count)
}
